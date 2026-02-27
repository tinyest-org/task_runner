# Plan: Retry with Backoff

## Objectif

Quand une tache echoue (timeout, webhook on_start KO, ou echec explicite via PATCH), la re-tenter automatiquement N fois avec un delai croissant, au lieu de la marquer comme definitivement echouee.

Aujourd'hui : echec = terminal. L'appelant doit re-creer la tache manuellement.

---

## Design

### 1. Retry policy sur le DTO

Ajouter un champ optionnel `retry` a `NewTaskDto` :

```rust
// dtos.rs
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct NewTaskDto {
    // ... champs existants ...

    /// Politique de retry. Si absent, pas de retry (comportement actuel).
    pub retry: Option<RetryPolicy>,
}

#[derive(Debug, Serialize, Deserialize, Clone, ToSchema)]
pub struct RetryPolicy {
    /// Nombre max de tentatives (hors tentative initiale).
    /// Ex: 3 = 1 tentative initiale + 3 retries = 4 executions max.
    pub max_retries: i32,

    /// Delai initial avant le premier retry (en secondes).
    /// Defaut: 5
    #[serde(default = "default_initial_delay")]
    pub initial_delay_secs: i32,

    /// Multiplicateur de backoff. Le delai est multiplie par ce facteur a chaque retry.
    /// Defaut: 2.0 (doublement a chaque retry)
    #[serde(default = "default_backoff_multiplier")]
    pub backoff_multiplier: f64,

    /// Delai maximum entre deux retries (en secondes). Cap le backoff exponentiel.
    /// Defaut: 300 (5 minutes)
    #[serde(default = "default_max_delay")]
    pub max_delay_secs: i32,

    /// Quels types d'echec declenchent un retry.
    /// Defaut: tous (timeout, webhook_failure, explicit)
    #[serde(default)]
    pub retry_on: Option<Vec<RetryTrigger>>,
}

#[derive(Debug, Serialize, Deserialize, Clone, ToSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RetryTrigger {
    /// Retry sur timeout (timeout_loop)
    Timeout,
    /// Retry quand le webhook on_start echoue
    WebhookFailure,
    /// Retry quand la tache est marquee Failure via PATCH
    Explicit,
}
```

### 2. Nouveaux champs sur `task`

```sql
-- Migration
ALTER TABLE task ADD COLUMN retry_policy JSONB;          -- NULL = pas de retry
ALTER TABLE task ADD COLUMN attempt INT NOT NULL DEFAULT 0;  -- tentative courante (0 = premiere)
ALTER TABLE task ADD COLUMN next_retry_at TIMESTAMPTZ;   -- quand retenter (NULL = pas de retry en attente)
```

```rust
// models.rs — ajouter a Task
pub struct Task {
    // ... existant ...
    pub retry_policy: Option<serde_json::Value>,  // RetryPolicy serialisee
    pub attempt: i32,
    pub next_retry_at: Option<chrono::DateTime<Utc>>,
}
```

### 3. Nouveau statut : `RetryPending`

```rust
pub enum StatusKind {
    // ... existant ...
    RetryPending,  // NEW — en attente de retry, ne sera pas pick up avant next_retry_at
}
```

```sql
ALTER TYPE status_kind ADD VALUE 'retry_pending';
```

`RetryPending` est un etat NON terminal. Le task reste "vivant" et ne propage PAS aux enfants.

### 4. Logique de retry — quand une tache echoue

Partout ou on marque une tache comme `Failure`, ajouter un check retry :

```
fn should_retry(task, failure_type) -> Option<Duration>:
    policy = task.retry_policy?
    if task.attempt >= policy.max_retries:
        return None  // plus de retries

    if let Some(triggers) = policy.retry_on:
        if !triggers.contains(failure_type):
            return None  // ce type d'echec n'est pas retryable

    delay = policy.initial_delay_secs * (policy.backoff_multiplier ^ task.attempt)
    delay = min(delay, policy.max_delay_secs)
    return Some(Duration::from_secs(delay))
```

**3 points d'injection** dans le code existant :

#### a) `timeout_loop` (timeout_loop.rs)
Quand `timeout_task_and_propagate` detecte un timeout :
- Avant de marquer Failure, verifier `should_retry(task, Timeout)`
- Si retry : `status = RetryPending`, `attempt += 1`, `next_retry_at = now + delay`, PAS de propagation
- Si pas de retry : comportement actuel (Failure + propagation)

#### b) `start_loop` (start_loop.rs)
Quand le webhook on_start echoue (`fail_task_and_propagate`) :
- Avant de marquer Failure, verifier `should_retry(task, WebhookFailure)`
- Si retry : `status = RetryPending`, `attempt += 1`, `next_retry_at = now + delay`
- Si pas de retry : comportement actuel

#### c) `update_running_task` (task_lifecycle.rs)
Quand l'appelant envoie `PATCH /task/{id}` avec `status: Failure` :
- Apres le update, verifier `should_retry(task, Explicit)`
- Si retry : au lieu de laisser en Failure, transition vers `RetryPending`, `attempt += 1`, `next_retry_at = now + delay`, PAS de propagation aux enfants
- Si pas de retry : comportement actuel (propagation)

### 5. Retry loop — nouveau worker loop

Nouveau worker (ou extension du start_loop) qui pick up les taches en `RetryPending` :

```rust
// workers/retry_loop.rs (nouveau fichier)

pub async fn retry_loop(pool, interval, shutdown):
    loop:
        conn = pool.get()
        // Trouver les taches RetryPending dont next_retry_at <= now
        retryable = SELECT * FROM task
            WHERE status = 'retry_pending'
            AND next_retry_at <= now()
            ORDER BY next_retry_at ASC
            FOR UPDATE SKIP LOCKED

        for task in retryable:
            // Reinitialiser la tache pour re-execution
            UPDATE task SET
                status = 'pending',
                started_at = NULL,
                ended_at = NULL,
                failure_reason = NULL,
                last_updated = now()
            WHERE id = task.id

            metrics::record_task_retry(task.attempt)

        sleep(interval)
```

**Pourquoi un loop separe plutot qu'integrer au start_loop ?**
- Le start_loop tourne avec son propre intervalle et fait deja du travail lourd (concurrency checks, webhook calls)
- Le retry_loop est un simple scan + update, tres leger
- Separation des responsabilites : plus facile a monitorer et ajuster independamment

### 6. Reset de l'etat au retry

Quand une tache passe de `RetryPending` a `Pending` :
- `status` = `Pending`
- `started_at` = NULL
- `ended_at` = NULL
- `failure_reason` = NULL
- `last_updated` = now()
- `success` et `failures` (batch counters) : **PAS remis a zero** (ils representent le cumul, pas la tentative courante)
- `attempt` : **PAS modifie** (deja incremente lors du passage en RetryPending)

Les actions (webhooks) restent identiques — les memes on_start/on_success/on_failure sont re-executees.

**Idempotency concern** : le `webhook_execution` record du precedent on_start est deja en status `success` ou `failure`. Il faut soit :
- Generer une nouvelle `idempotency_key` incluant le numero d'attempt (ex: `{task_id}:start:success:attempt-{N}`)
- Ou supprimer le record existant avant le retry

**Recommandation** : inclure `attempt` dans la `idempotency_key`. C'est la solution la plus propre. Modifier `idempotency_key()` dans `action.rs` :
```rust
fn idempotency_key(task_id: Uuid, trigger: &TriggerKind, condition: &TriggerCondition, attempt: i32) -> String {
    format!("{}:{}:{}:{}", task_id, trigger, condition, attempt)
}
```

### 7. Impact sur la propagation

**Regle fondamentale** : un task en `RetryPending` ne propage PAS aux enfants. La propagation ne se fait que quand le task atteint un etat terminal definitif (Success ou Failure apres epuisement des retries).

Cela signifie :
- Les enfants restent en `Waiting` pendant que le parent retente
- Le timeout du parent est reset a chaque retry (via reset de `last_updated`)
- Si le parent finit par reussir apres N retries, la propagation Success se fait normalement

### 8. Impact sur les fonctionnalites existantes

| Composant               | Impact                                                              |
|--------------------------|---------------------------------------------------------------------|
| `propagate_to_children`  | Pas appele quand transition vers `RetryPending` (uniquement sur terminal) |
| `cancel_task`            | `RetryPending` doit etre cancelable (comme Pending)                  |
| `pause_task`             | `RetryPending` doit etre pausable                                    |
| `stop_batch`             | `RetryPending` inclus dans les taches a cancel                      |
| `dead_end_ancestors`     | `RetryPending` = NON terminal, donc pas inclus dans les checks       |
| `batch_updater`          | Pas d'impact (ne concerne que les Running tasks)                     |
| `concurrency rules`      | Pas d'impact (`RetryPending` n'est pas Running, pas compte)          |
| `list/filter`            | `RetryPending` disponible comme valeur de filtre                     |
| DAG visualization        | Afficher le statut RetryPending, numero d'attempt, next_retry_at     |
| Metriques                | `task_retry_total`, `task_retry_exhausted_total`, retry par attempt number |

### 9. Modifications des DTOs de sortie

`TaskDto` et `BasicTaskDto` : ajouter les champs retry.

```rust
pub struct TaskDto {
    // ... existant ...
    pub retry: Option<RetryPolicyDto>,  // politique configuree
    pub attempt: i32,                   // tentative courante
    pub next_retry_at: Option<chrono::DateTime<Utc>>,
}

pub struct BasicTaskDto {
    // ... existant ...
    pub attempt: i32,
}
```

### 10. Configuration globale

Ajouter des env vars pour borner les politiques de retry :

```
RETRY_MAX_RETRIES_LIMIT=10       # max_retries ne peut pas depasser cette valeur
RETRY_MAX_DELAY_LIMIT=3600       # max_delay_secs ne peut pas depasser 1h
RETRY_LOOP_INTERVAL_MS=1000      # intervalle du retry loop
```

### 11. Validation

Dans `validate_task_batch` :
- `max_retries` doit etre >= 1 et <= `RETRY_MAX_RETRIES_LIMIT`
- `initial_delay_secs` doit etre >= 1
- `backoff_multiplier` doit etre >= 1.0
- `max_delay_secs` doit etre >= `initial_delay_secs` et <= `RETRY_MAX_DELAY_LIMIT`
- `retry_on` si present ne doit pas etre vide

### 12. Exemple d'utilisation

```json
POST /task
[
  {
    "id": "import",
    "name": "Import Data",
    "kind": "etl",
    "timeout": 120,
    "retry": {
      "max_retries": 3,
      "initial_delay_secs": 10,
      "backoff_multiplier": 2.0,
      "max_delay_secs": 120
    },
    "on_start": {"kind": "Webhook", "params": {"url": "https://etl/import", "verb": "Post"}}
  }
]
```

Timeline si les 2 premieres tentatives echouent et la 3eme reussit :
```
t=0     import: Pending → Claimed → Running (attempt=0)
t=30    import: timeout → RetryPending (attempt=1, next_retry_at=t+10)
t=40    import: RetryPending → Pending → Claimed → Running
t=70    import: timeout → RetryPending (attempt=2, next_retry_at=t+20)
t=90    import: RetryPending → Pending → Claimed → Running
t=100   import: PATCH Success → Success (attempt=2) → propagation aux enfants
```

### 13. Ordre d'implementation

1. **Migration DB** : `retry_policy`, `attempt`, `next_retry_at` columns, `retry_pending` status value
2. **Models/Schema** : mettre a jour `Task`, `NewTask`, `StatusKind`, regenerer schema.rs
3. **DTOs** : `RetryPolicy`, `RetryTrigger`, serializers, ajouter `attempt` aux DTOs de sortie
4. **Validation** : valider `RetryPolicy` dans `validate_task_batch`
5. **Config** : env vars pour les limites globales
6. **Core** : fonction `should_retry(task, failure_type) -> Option<Duration>`
7. **Idempotency key** : inclure `attempt` dans la generation
8. **Points d'injection** :
   a. `timeout_task_and_propagate` → check retry avant Failure
   b. `fail_task_and_propagate` → check retry avant Failure (webhook failure)
   c. `update_running_task` → check retry avant propagation (explicit failure)
9. **Retry loop** : nouveau `workers/retry_loop.rs`, spawn dans main.rs
10. **Cancel/Pause** : ajouter `RetryPending` aux etats valides
11. **Stop batch** : inclure `RetryPending` dans les cancel
12. **Metriques** : nouveaux compteurs
13. **Tests** :
    - Unit : `should_retry` avec differentes policies et failure types
    - Unit : calcul du backoff delay (exponentiel, cap)
    - Integration : retry sur timeout → eventually succeeds
    - Integration : retry sur webhook failure → eventually succeeds
    - Integration : retry exhausted → Failure + propagation aux enfants
    - Integration : cancel pendant RetryPending
    - Integration : retry ne propage pas aux enfants (enfants restent Waiting)
    - Integration : idempotency key avec attempts differents
14. **DAG visualization** : affichage attempt/retry status

### 14. Risques

- **Retry + propagation** : il faut etre certain que la transition vers `RetryPending` ne declenche JAMAIS de propagation. Un seul oubli et les enfants avancent alors que le parent va re-tenter.
- **Idempotency** : les records webhook_execution doivent etre distincts par attempt, sinon le retry sera skip par le guard idempotent.
- **Resource consumption** : une tache qui retry 10 fois avec un timeout de 5 min consomme 50 min de "slot". Les concurrency rules la comptent comme Running pendant ce temps. Le `RetryPending` state resout ce probleme (pas compte comme Running).
- **Interaction avec cancel** : si un utilisateur cancel un task en `RetryPending`, c'est un cancel definitif (pas de re-retry).
- **Interaction avec pause** : si un utilisateur pause un task en `RetryPending`, il passe en `Paused`. Au un-pause, il revient en `RetryPending` (pas en Pending — il doit attendre son `next_retry_at`).
