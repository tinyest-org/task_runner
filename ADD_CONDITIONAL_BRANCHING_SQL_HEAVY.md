# Plan: Conditional Branching avec propagation SQL-heavy

## Contexte

Le projet ADD_CONDITIONAL_BRANCHING.md décrit le remplacement de `requires_success: bool` par `condition: always|on_success|on_failure` sur les dépendances du DAG. Ce plan raffine l'implémentation en **poussant la classification et les updates côté SQL** plutôt que côté Rust.

**Problème actuel** : `propagate_to_children` fait 1 SELECT + une boucle Rust de classification + 4 UPDATEs + des appels récursifs Rust par enfant échoué = **5+ round-trips SQL + logique Rust**.

**Objectif** : **1 seule requête CTE writable par niveau de propagation** + une boucle Rust itérative (non récursive) pour le cascade.

---

## 1. Migration DB

Fichier : `migrations/XXXX_conditional_branching/up.sql`

```sql
ALTER TYPE status_kind ADD VALUE IF NOT EXISTS 'skipped';

CREATE TYPE dependency_condition AS ENUM ('always', 'on_success', 'on_failure');

ALTER TABLE link ADD COLUMN condition dependency_condition NOT NULL DEFAULT 'always';
UPDATE link SET condition = CASE
    WHEN requires_success = true THEN 'on_success'::dependency_condition
    ELSE 'always'::dependency_condition
END;
ALTER TABLE link DROP COLUMN requires_success;

ALTER TABLE task ADD COLUMN wait_on_failure INT NOT NULL DEFAULT 0;
```

---

## 2. Models & Schema

### `src/schema.rs`
- Ajouter `DependencyCondition` dans `sql_types`
- Mettre à jour `link` : remplacer `requires_success -> Bool` par `condition -> DependencyCondition`
- Ajouter `wait_on_failure -> Int4` à `task`

### `src/models.rs`
- Ajouter enum `DependencyCondition { Always, OnSuccess, OnFailure }` (derive `DbEnum`)
- Ajouter `Skipped` à `StatusKind`
- `Link` : remplacer `requires_success: bool` par `condition: DependencyCondition`
- `Task` / `NewTask` : ajouter `wait_on_failure: i32`

---

## 3. DTOs (`src/dtos.rs`)

### `Dependency` — backward compat
```rust
pub struct Dependency {
    pub id: String,
    pub condition: Option<DependencyCondition>,  // nouveau
    pub requires_success: Option<bool>,          // ancien, deprecated
}

impl Dependency {
    pub fn resolved_condition(&self) -> DependencyCondition {
        if let Some(c) = self.condition { return c; }
        match self.requires_success {
            Some(true) => DependencyCondition::OnSuccess,
            _ => DependencyCondition::Always,
        }
    }
}
```

### Autres DTOs
- `LinkDto` : remplacer `requires_success` par `condition: DependencyCondition`
- `BatchStatusCounts` : ajouter `skipped: i64`
- `TaskDto` / `BasicTaskDto` : pas de changement structurel

---

## 4. Insertion des tâches (`src/db/task_crud.rs`)

### `insert_new_task` — calcul des compteurs

Remplacer le bloc lignes 131-155 :

```rust
for dep in deps {
    let cond = dep.resolved_condition();
    match cond {
        DependencyCondition::Always    => wait_finished += 1,
        DependencyCondition::OnSuccess => wait_success += 1,
        DependencyCondition::OnFailure => wait_on_failure += 1,
    }
    resolved_links.push(Link { parent_id, child_id: Uuid::nil(), condition: cond });
}
```

### Statut initial (ligne 158)

```rust
let initial_status = if (wait_finished + wait_success + wait_on_failure) > 0 {
    StatusKind::Waiting
} else {
    StatusKind::Pending
};
```

---

## 5. Propagation SQL-heavy (`src/workers/propagation.rs`)

### CTE unique — 1 requête par niveau

```sql
-- $1 = UUID[] parent_ids, $2 = TEXT parent_status
WITH
children AS (
    SELECT l.child_id, l.condition::text AS cond
    FROM link l WHERE l.parent_id = ANY($1)
),
classified AS (
    SELECT child_id, cond,
        CASE
            WHEN cond = 'always' THEN true
            WHEN cond = 'on_success' AND $2 = 'success' THEN true
            WHEN cond = 'on_failure' AND $2 IN ('failure', 'canceled') THEN true
            ELSE false
        END AS is_satisfied
    FROM children
),
children_to_skip AS (
    SELECT DISTINCT child_id FROM classified WHERE NOT is_satisfied
),
counter_decrements AS (
    SELECT child_id,
        COUNT(*) FILTER (WHERE cond = 'always')::int      AS dec_finished,
        COUNT(*) FILTER (WHERE cond = 'on_success')::int   AS dec_success,
        COUNT(*) FILTER (WHERE cond = 'on_failure')::int    AS dec_failure
    FROM classified
    WHERE child_id NOT IN (SELECT child_id FROM children_to_skip)
    GROUP BY child_id
),
mark_skipped AS (
    UPDATE task SET status = 'skipped',
        failure_reason = 'Dependency condition not met',
        ended_at = now(), last_updated = now()
    WHERE id IN (SELECT child_id FROM children_to_skip) AND status = 'waiting'
    RETURNING id
),
update_counters AS (
    UPDATE task SET
        wait_finished   = task.wait_finished   - cd.dec_finished,
        wait_success    = task.wait_success    - cd.dec_success,
        wait_on_failure = task.wait_on_failure - cd.dec_failure,
        status = CASE
            WHEN (task.wait_finished - cd.dec_finished) <= 0
             AND (task.wait_success  - cd.dec_success)  <= 0
             AND (task.wait_on_failure - cd.dec_failure) <= 0
            THEN 'pending'::status_kind ELSE task.status END
    FROM counter_decrements cd
    WHERE task.id = cd.child_id AND task.status = 'waiting'
    RETURNING task.id, task.status::text AS new_status
)
SELECT id, 'skipped'::text AS action FROM mark_skipped
UNION ALL
SELECT id, new_status AS action FROM update_counters;
```

**Sémantique CTE writable** : `mark_skipped` et `update_counters` opèrent sur des ensembles disjoints (un enfant est soit skip, soit décrémenté, jamais les deux). PostgreSQL voit le même snapshot pour les deux, ce qui est correct ici.

### Boucle Rust — itérative, non récursive

```rust
pub(crate) async fn propagate_to_children(
    parent_id: &Uuid, result_status: &StatusKind, conn: &mut Conn<'_>,
) -> Result<Vec<Uuid>, DbError> {
    let status_str = match result_status {
        StatusKind::Success  => "success",
        StatusKind::Failure  => "failure",
        StatusKind::Canceled => "canceled",
        StatusKind::Skipped  => "skipped",
        _ => return Ok(vec![]),
    };

    let mut all_skipped = Vec::new();
    let mut current_parents = vec![*parent_id];
    let mut current_status = status_str.to_string();

    loop {
        if current_parents.is_empty() { break; }

        let rows = diesel::sql_query(PROPAGATION_CTE)
            .bind::<Array<Uuid>, _>(&current_parents)
            .bind::<Text, _>(&current_status)
            .load::<PropagationRow>(conn).await?;

        let skipped: Vec<Uuid> = rows.iter()
            .filter(|r| r.action == "skipped").map(|r| r.id).collect();
        let pending: Vec<Uuid> = rows.iter()
            .filter(|r| r.action == "pending").map(|r| r.id).collect();

        // Métriques
        for _ in &skipped { metrics::record_task_skipped(); }
        for _ in &pending  { metrics::record_task_unblocked(); }

        if skipped.is_empty() { break; }

        all_skipped.extend(&skipped);
        current_parents = skipped;
        current_status = "skipped".to_string();
    }
    Ok(all_skipped)
}
```

### Comparaison avant/après

| Aspect | Avant | Après |
|--------|-------|-------|
| Queries par niveau | 1 SELECT + 4 UPDATE | 1 CTE writable |
| Classification | Boucle Rust | `CASE` SQL dans `classified` |
| Récursion | `Box::pin(propagate_to_children(...))` par enfant | Boucle `loop` plate sur le batch d'IDs |
| Décréments multiples parents | Impossible (1 parent par appel) | `SUM()` agrège si plusieurs parents |

---

## 6. Webhooks pour les tâches Skipped

**Les tâches Skipped ne déclenchent AUCUN webhook** : elles n'ont jamais été started, donc pas de on_start/on_success/on_failure/cancel.

Impact sur les callers de `propagate_to_children` :
- `update_running_task` (task_lifecycle.rs:114) : `fire_end_webhooks_with_cascade` — la liste retournée est maintenant des skipped IDs. **Ne pas** fire de webhooks pour eux. Simplifier : ne fire que le webhook du parent lui-même.
- `fail_task_and_propagate` (task_lifecycle.rs:248) : idem.
- `timeout_task_and_propagate` (task_query.rs:81) : idem.

**Changement de comportement** : aujourd'hui, les enfants cascade-failed reçoivent des on_failure webhooks. Avec Skipped, ils n'en reçoivent plus. C'est intentionnel : un task Skipped n'a jamais existé du point de vue de l'exécuteur.

---

## 7. Dead-end detection (`cancel_dead_end_ancestors`)

Ajouter `'skipped'` aux filtres d'états terminaux dans le CTE SQL (lignes 354-360 de propagation.rs) :

```sql
WHERE t.status NOT IN ('success', 'failure', 'canceled', 'skipped')
-- et
AND c2.status NOT IN ('success', 'failure', 'canceled', 'skipped')
```

---

## 8. Cleanup (`src/db/cleanup.rs`)

Ajouter `Skipped` au filtre terminal (ligne 23-26) :

```rust
.or(task_dsl::status.eq(StatusKind::Skipped))
```

---

## 9. Batch listing (`src/db/batch_listing.rs`)

Le pivot Rust `BatchStatusRow.status_text` → `BatchStatusCounts` doit mapper `"skipped"` vers `counts.skipped`.

---

## 10. DAG query (`src/db/task_query.rs`)

`get_dag_for_batch` (ligne 214-221) : mettre à jour le mapping `Link` → `LinkDto` pour utiliser `condition` au lieu de `requires_success`.

---

## 11. Validation (`src/validation/task.rs`)

- Valider que `condition` et `requires_success` ne sont pas fournis simultanément sur la même dépendance
- Valider que `condition` est une valeur valide (serde le fait, mais log un warning si `requires_success` est utilisé : "deprecated, use condition instead")

---

## 12. Métriques (`src/metrics.rs`)

- Ajouter `record_task_skipped()`
- Ajouter `"Skipped"` dans les labels de `record_status_transition`

---

## 13. Ordre d'implémentation

1. Migration DB
2. `schema.rs` (regenerate)
3. `models.rs` (DependencyCondition, Skipped, Link, Task)
4. `dtos.rs` (Dependency backward compat, LinkDto, BatchStatusCounts)
5. `validation/task.rs`
6. `db/task_crud.rs` (insertion counters)
7. **`workers/propagation.rs`** (CTE + boucle itérative) — le gros morceau
8. `db/task_lifecycle.rs` (webhook logic pour skipped)
9. `db/task_query.rs` (DAG query, timeout propagation)
10. `db/cleanup.rs` + `db/batch_listing.rs`
11. `metrics.rs`
12. Tests helpers + integration tests
13. `static/dag.html`

---

## 14. Tests

### Nouveaux tests d'intégration (`tests/test_conditional_branching.rs`)

| Test | Scénario |
|------|----------|
| `test_on_success_activates` | Parent Success → child on_success → Pending |
| `test_on_failure_activates` | Parent Failure → child on_failure → Pending |
| `test_on_success_skipped` | Parent Failure → child on_success → Skipped |
| `test_on_failure_skipped` | Parent Success → child on_failure → Skipped |
| `test_always_any_outcome` | Parent Success ou Failure → child always → Pending |
| `test_cascade_skip` | Root Failure → A(on_success) Skipped → B(on_success de A) Skipped |
| `test_mixed_notify_pattern` | build → deploy(on_success) + rollback(on_failure) → notify(always) |
| `test_multi_parent_mixed` | Child dépend de A(on_success) + B(on_failure). A Success, B Failure → Pending |
| `test_skipped_decrements_always` | A Skipped → B(always de A) voit wait_finished décrementé |
| `test_backward_compat_requires_success` | Ancien JSON `requires_success: true` → comportement on_success |
| `test_no_webhooks_for_skipped` | Task Skipped → aucun webhook fired |
| `test_skipped_terminal_for_dead_end` | Skipped compté comme terminal pour dead-end detection |

### Tests existants à mettre à jour
- `test_propagation.rs` : les enfants avec `requires_success=true` dont le parent échoue deviennent `Skipped` (et non plus `Failure`)
- `test_bug_audit1.rs` / `test_bug_audit2.rs` : vérifier que les regressions passent toujours
- `tests/common/builders.rs` : ajouter helper `task_with_condition(id, deps_with_conditions)`

---

## 15. Risques

| Risque | Sévérité | Mitigation |
|--------|----------|------------|
| **Changement sémantique Failure → Skipped** pour les cascade-failed children | Haut | Tests exhaustifs. Documenter le changement dans le CHANGELOG. API consumers qui filtrent sur `Failure` doivent aussi checker `Skipped`. |
| **Snapshot CTE writable** : mark_skipped et update_counters voient le même état | Moyen | Safe car ensembles disjoints. Tester avec cas multi-parent. |
| **Counter `<= 0` au lieu de `= 0`** dans la transition Pending | Moyen | Le `<= 0` est défensif contre les double-decrements. Mais ne devrait jamais arriver. Utiliser `= 0` + assertion/log si < 0. |
| **Backward compat** serde pour Dependency | Bas | Tester les deux formats JSON. |
