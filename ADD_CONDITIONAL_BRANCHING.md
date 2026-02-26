# Plan: Conditional Branching

## Objectif

Permettre de router l'execution dans le DAG en fonction du resultat d'une tache parente.
Exemple : "si build reussit, lancer deploy ; si build echoue, lancer notify-failure."

Aujourd'hui le seul mecanisme est `requires_success` sur `Dependency`, qui est binaire :
- `true` = "si parent echoue, je suis marque en echec"
- `false` = "j'attends juste que parent finisse"

Il manque : "ne m'execute QUE si parent echoue" (branche erreur).

---

## Design

### 1. Remplacer `requires_success: bool` par `condition: enum`

```rust
// dtos.rs
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct Dependency {
    pub id: String,
    /// Quand cette dependance est consideree comme satisfaite.
    /// - "always"     : parent termine (quel que soit le resultat) — defaut
    /// - "on_success" : parent termine avec Success uniquement
    /// - "on_failure" : parent termine avec Failure ou Canceled uniquement
    #[serde(default = "default_condition")]
    pub condition: DependencyCondition,
}

#[derive(Debug, Serialize, Deserialize, ToSchema, Clone, Copy, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DependencyCondition {
    Always,
    OnSuccess,
    OnFailure,
}

fn default_condition() -> DependencyCondition {
    DependencyCondition::Always
}
```

**Compatibilite arriere** : l'ancien format `{ "id": "x", "requires_success": true }` doit continuer a fonctionner. Strategie :
- Si le JSON contient `condition`, l'utiliser
- Si le JSON contient `requires_success`, le mapper : `true` → `OnSuccess`, `false` → `Always`
- Implementer via un deserializer custom ou `#[serde(alias)]` + logique de normalisation dans la validation

Mapping :
| Ancien                  | Nouveau          |
|-------------------------|------------------|
| `requires_success=true` | `OnSuccess`      |
| `requires_success=false`| `Always`         |
| (nouveau)               | `OnFailure`      |

### 2. Nouveau statut : `Skipped`

Quand une dependance conditionnelle ne peut pas etre satisfaite (ex: parent reussit mais le lien est `on_failure`), la tache enfant est marquee `Skipped`.

```rust
// models.rs — ajouter au enum StatusKind
pub enum StatusKind {
    Waiting,
    Pending,
    Claimed,
    Running,
    Failure,
    Success,
    Paused,
    Canceled,
    Skipped,  // NEW
}
```

`Skipped` est un etat terminal. Pour la propagation, il est traite comme `Canceled` :
les enfants d'un task Skipped avec `on_success` sont eux aussi Skipped (cascade).

### 3. Modification du schema `link`

```sql
-- Migration: replace requires_success with condition enum

CREATE TYPE dependency_condition AS ENUM ('always', 'on_success', 'on_failure');

ALTER TABLE link ADD COLUMN condition dependency_condition NOT NULL DEFAULT 'always';

-- Migrate existing data
UPDATE link SET condition = CASE
    WHEN requires_success = true THEN 'on_success'::dependency_condition
    ELSE 'always'::dependency_condition
END;

ALTER TABLE link DROP COLUMN requires_success;
```

Ajouter aussi la valeur `skipped` au type enum `status_kind` :
```sql
ALTER TYPE status_kind ADD VALUE 'skipped';
```

### 4. Modification des compteurs sur `task`

Remplacer `wait_success: i32` par `wait_on_failure: i32` :

| Compteur         | Signification                                                |
|------------------|--------------------------------------------------------------|
| `wait_finished`  | Nombre de deps `always` restantes                            |
| `wait_success`   | Nombre de deps `on_success` restantes (inchange de semantique)|
| `wait_on_failure` | Nombre de deps `on_failure` restantes (NEW)                  |

Transition vers `Pending` quand les TROIS compteurs sont a 0.

```sql
ALTER TABLE task ADD COLUMN wait_on_failure INT NOT NULL DEFAULT 0;
```

### 5. Modification de `propagate_to_children`

Le coeur du changement. Pseudo-code :

```
fn propagate_to_children(parent_id, result_status, conn):
    children_links = get_links(parent_id)  // (child_id, condition)

    pour chaque (child_id, condition) dans children_links:
        match (result_status, condition):

            // Parent reussit
            (Success, Always):
                decrementer wait_finished du child
            (Success, OnSuccess):
                decrementer wait_success du child
            (Success, OnFailure):
                marquer child comme Skipped ("Parent succeeded, on_failure condition not met")
                propager recursivement aux enfants du child

            // Parent echoue / canceled
            (Failure|Canceled, Always):
                decrementer wait_finished du child
            (Failure|Canceled, OnFailure):
                decrementer wait_on_failure du child
            (Failure|Canceled, OnSuccess):
                marquer child comme Skipped ("Parent failed, on_success condition not met")
                propager recursivement aux enfants du child

    // Transition vers Pending les children ou les 3 compteurs sont a 0
    UPDATE task SET status = 'pending'
    WHERE id IN (remaining_child_ids)
      AND status = 'waiting'
      AND wait_finished = 0
      AND wait_success = 0
      AND wait_on_failure = 0
```

**Important** : la propagation recursive depuis un task `Skipped` suit les memes regles que `Canceled` : les enfants `on_success` sont cascade-skipped, les enfants `always` et `on_failure` voient leur compteur decremente normalement.

### 6. Calcul des compteurs a l'insertion

Dans `insert_new_task` (task_crud.rs), quand on cree les liens :

```
pour chaque dependency dans task.dependencies:
    match dependency.condition:
        Always     => wait_finished += 1
        OnSuccess  => wait_success += 1
        OnFailure  => wait_on_failure += 1
```

Si aucune dependency : le task commence en `Pending` (inchange).

### 7. Modifications des DTOs de sortie

`LinkDto` :
```rust
pub struct LinkDto {
    pub parent_id: uuid::Uuid,
    pub child_id: uuid::Uuid,
    pub condition: DependencyCondition,  // remplace requires_success
}
```

### 8. Impact sur les fonctionnalites existantes

| Composant               | Impact                                                         |
|--------------------------|----------------------------------------------------------------|
| `cancel_task`            | Inchange (`Canceled` propage comme `Failure`)                  |
| `timeout_loop`           | Inchange (timeout → Failure → propagation normale)             |
| `start_loop`             | Inchange (ne touche pas les deps)                              |
| `batch_updater`          | Inchange (compteurs success/failures independants)             |
| `stop_batch`             | Inchange (cancel tout, `Skipped` pas concerne)                 |
| `dead_end_ancestors`     | Ajouter `Skipped` a la liste des etats terminaux               |
| `cancel_dead_end_ancestors` | `Skipped` = terminal, donc inclus dans les checks           |
| `validate_task_batch`    | Valider `condition` sur les deps, detecter cycles              |
| DAG visualization        | Afficher la condition sur les aretes, colorer les tasks Skipped|
| Metriques                | Ajouter `task_skipped_total`, `status_transition` pour Skipped |
| Filtrage/listing         | `Skipped` disponible comme valeur de filtre                    |

### 9. Validation

- Une tache ne peut pas avoir TOUTES ses deps en `on_failure` si elle n'a pas au moins une possibilite d'etre activee (warning, pas erreur — c'est un pattern valide pour un error-handler)
- Detecter les cas impossibles au sein du batch : si task A n'a pas d'action `on_failure` definie et que task B depend de A en `on_failure`, c'est probablement un bug utilisateur → warning dans les logs mais pas une erreur bloquante (A peut echouer pour d'autres raisons)

### 10. Exemple d'utilisation

```json
POST /task
[
  {
    "id": "build",
    "name": "Build",
    "kind": "ci",
    "on_start": {"kind": "Webhook", "params": {"url": "https://ci/build", "verb": "Post"}}
  },
  {
    "id": "deploy",
    "name": "Deploy (on success)",
    "kind": "ci",
    "dependencies": [{"id": "build", "condition": "on_success"}],
    "on_start": {"kind": "Webhook", "params": {"url": "https://ci/deploy", "verb": "Post"}}
  },
  {
    "id": "rollback",
    "name": "Rollback (on failure)",
    "kind": "ci",
    "dependencies": [{"id": "build", "condition": "on_failure"}],
    "on_start": {"kind": "Webhook", "params": {"url": "https://ci/rollback", "verb": "Post"}}
  },
  {
    "id": "notify",
    "name": "Notify (always)",
    "kind": "ci",
    "dependencies": [
      {"id": "deploy", "condition": "always"},
      {"id": "rollback", "condition": "always"}
    ],
    "on_start": {"kind": "Webhook", "params": {"url": "https://slack/notify", "verb": "Post"}}
  }
]
```

Scenario si build reussit : `build(Success)` → `deploy(Pending→Running)` + `rollback(Skipped)` → `notify(Pending→Running)`
Scenario si build echoue : `build(Failure)` → `deploy(Skipped)` + `rollback(Pending→Running)` → `notify(Pending→Running)`

### 11. Ordre d'implementation

1. **Migration DB** : ajouter `dependency_condition` enum, `wait_on_failure` column, `skipped` status value, migrer `requires_success` → `condition`
2. **Models/Schema** : mettre a jour `Link`, `NewTask`, `StatusKind`, regenerer schema.rs
3. **DTOs** : `DependencyCondition` enum, backward compat deserializer, `LinkDto` update
4. **Validation** : valider `condition` dans `validate_task_batch`
5. **Insertion** : modifier `insert_new_task` pour calculer `wait_on_failure`
6. **Propagation** : refactorer `propagate_to_children` avec la logique a 3 voies
7. **Dead-end detection** : ajouter `Skipped` aux etats terminaux
8. **Metriques** : nouveaux compteurs
9. **Tests** :
   - Unit tests pour `propagate_to_children` avec chaque combinaison condition × resultat
   - Integration tests : DAG avec branches success/failure, cascade skip, mixed conditions
   - Backward compat : ancien format JSON `requires_success` toujours accepte
10. **DAG visualization** : affichage des conditions sur les aretes

### 12. Risques

- **Complexite de propagation** : la logique a 3 voies (always/on_success/on_failure) multiplie les cas. Tests exhaustifs indispensables.
- **`notify` dans l'exemple** : si `deploy` est Skipped et `rollback` termine, notify doit comprendre que ses deux deps sont resolues (l'une par completion, l'autre par skip). `Skipped` doit decrementer les compteurs `always` des enfants, comme le fait `Canceled`.
- **Backward compat** : la migration de `requires_success` → `condition` doit etre testee sur les donnees existantes.
