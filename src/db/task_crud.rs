use crate::{
    Conn,
    dtos::{self, TaskDto},
    metrics,
    models::{self, Action, Link, NewAction, StatusKind, Task},
    rule::{CapacityRule, Matcher, Strategy},
};
use diesel::prelude::*;
use diesel_async::RunQueryDsl;
use serde_json::json;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use uuid::Uuid;

use super::{DbError, run_in_transaction};

/// ensure we avoid creating duplicate tasks
async fn handle_dedupe<'a>(
    conn: &mut Conn<'a>,
    rules: Vec<Matcher>,
    _metadata: &Option<serde_json::Value>,
) -> Result<bool, DbError> {
    for matcher in rules.iter() {
        use crate::schema::task::dsl::*;
        use diesel::PgJsonbExpressionMethods;
        let mut m = json!({});
        if let Some(_m) = _metadata {
            let mut fields_ok = true;
            for e in &matcher.fields {
                let k = _m.get(e);
                match k {
                    Some(v) => {
                        m[e] = v.clone();
                    }
                    None => {
                        log::warn!(
                            "Metadata missing field '{}' required by dedupe matcher, skipping rule",
                            e
                        );
                        fields_ok = false;
                        break;
                    }
                }
            }
            if !fields_ok {
                // Skip this rule (allow creation) since we can't evaluate it
                continue;
            }
        } else if !matcher.fields.is_empty() {
            // Metadata is None but the matcher requires field comparisons.
            // We can't evaluate this rule without metadata, so skip it
            // (allow creation). Without this guard, m stays as {} and
            // metadata.contains({}) would match ALL existing tasks with
            // non-null metadata, causing over-aggressive deduplication.
            log::warn!(
                "Metadata is None but dedupe matcher requires fields {:?}, skipping rule",
                matcher.fields
            );
            continue;
        }
        let count = task
            .filter(
                kind.eq(&matcher.kind)
                    .and(status.eq(&matcher.status))
                    .and(metadata.contains(m)),
            )
            .count()
            .get_result::<i64>(conn)
            .await?;
        if count > 0 {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(crate) async fn insert_actions<'a>(
    task_id: Uuid,
    actions: &[dtos::NewActionDto],
    trigger: &models::TriggerKind,
    condition: &models::TriggerCondition,
    conn: &mut Conn<'a>,
) -> Result<Vec<Action>, DbError> {
    use crate::schema::action::dsl::action;
    if actions.is_empty() {
        return Ok(vec![]);
    }
    let items = actions
        .iter()
        .map(|a| NewAction {
            task_id,
            kind: &a.kind,
            params: a.params.clone(),
            trigger,
            condition,
        })
        .collect::<Vec<_>>();

    let r = diesel::insert_into(action)
        .values(items)
        .returning(Action::as_returning())
        .get_results(conn)
        .await?;
    Ok(r)
}

/// Insert a new task into the database with optional dependencies.
///
/// `id_mapping` maps local client IDs to database UUIDs for resolving dependencies
/// within the same batch of tasks.
/// `batch_id` groups all tasks created in the same request for tracing.
pub(crate) async fn insert_new_task<'a>(
    conn: &mut Conn<'a>,
    dto: dtos::NewTaskDto,
    id_mapping: &HashMap<String, Uuid>,
    batch_id: Option<Uuid>,
) -> Result<Option<TaskDto>, DbError> {
    use crate::schema::link::dsl::link;
    use crate::schema::task::dsl::task;

    let should_write = if let Some(s) = dto.dedupe_strategy {
        handle_dedupe(conn, s, &dto.metadata).await?
    } else {
        true
    };

    if !should_write {
        return Ok(None);
    }

    // Compute wait counters from dependencies
    let (wait_success, wait_finished, links) = if let Some(ref deps) = dto.dependencies {
        let mut ws = 0i32;
        let mut wf = 0i32;
        let mut resolved_links = Vec::new();

        for dep in deps {
            if let Some(&parent_id) = id_mapping.get(&dep.id) {
                wf += 1;
                if dep.requires_success {
                    ws += 1;
                }
                resolved_links.push(Link {
                    parent_id,
                    child_id: Uuid::nil(), // Will be set after task creation
                    requires_success: dep.requires_success,
                });
            } else {
                log::warn!("Dependency with local id '{}' not found in mapping", dep.id);
            }
        }
        (ws, wf, resolved_links)
    } else {
        (0, 0, Vec::new())
    };

    // Set status based on whether there are dependencies
    let initial_status = if wait_finished > 0 {
        models::StatusKind::Waiting
    } else {
        models::StatusKind::Pending
    };

    let new_task = models::NewTask {
        name: dto.name,
        kind: dto.kind,
        status: initial_status,
        timeout: dto.timeout.unwrap_or(60),
        metadata: dto.metadata.unwrap_or(serde_json::Value::Null),
        start_condition: dto.rules.unwrap_or_default(),
        wait_success,
        wait_finished,
        batch_id,
        expected_count: dto.expected_count,
        dead_end_barrier: dto.dead_end_barrier.unwrap_or(false),
    };

    let new_task = diesel::insert_into(task)
        .values(new_task)
        .returning(Task::as_returning())
        .get_result(conn)
        .await?;

    // Insert links with the actual child_id
    if !links.is_empty() {
        let links_to_insert: Vec<Link> = links
            .into_iter()
            .map(|mut l| {
                l.child_id = new_task.id;
                l
            })
            .collect();

        diesel::insert_into(link)
            .values(&links_to_insert)
            .execute(conn)
            .await?;
    }

    // Insert actions: on_start (single), on_failure (many), on_success (many)
    let mut all_actions = Vec::new();

    // Insert on_start action (condition doesn't matter for start, use Success as default)
    let start_actions = insert_actions(
        new_task.id,
        &[dto.on_start],
        &models::TriggerKind::Start,
        &models::TriggerCondition::Success,
        conn,
    )
    .await?;
    all_actions.extend(start_actions);

    // Insert on_failure actions
    if let Some(failure_actions) = dto.on_failure {
        let inserted = insert_actions(
            new_task.id,
            &failure_actions,
            &models::TriggerKind::End,
            &models::TriggerCondition::Failure,
            conn,
        )
        .await?;
        all_actions.extend(inserted);
    }

    // Insert on_success actions
    if let Some(success_actions) = dto.on_success {
        let inserted = insert_actions(
            new_task.id,
            &success_actions,
            &models::TriggerKind::End,
            &models::TriggerCondition::Success,
            conn,
        )
        .await?;
        all_actions.extend(inserted);
    }

    // Record metrics
    metrics::record_task_created();
    if wait_finished > 0 {
        metrics::record_task_with_dependencies();
    }

    Ok(Some(TaskDto::new(new_task, all_actions)))
}

/// Find a task by ID with all its actions using a single LEFT JOIN query.
/// Returns None if the task doesn't exist.
pub(crate) async fn find_detailed_task_by_id<'a>(
    conn: &mut Conn<'a>,
    task_id: Uuid,
) -> Result<Option<dtos::TaskDto>, DbError> {
    use crate::schema::action::dsl as action_dsl;
    use crate::schema::task::dsl::*;

    // Use LEFT JOIN to fetch task and actions in a single query
    let results: Vec<(models::Task, Option<Action>)> = task
        .left_join(action_dsl::action)
        .filter(id.eq(task_id))
        .load::<(models::Task, Option<Action>)>(conn)
        .await?;

    if results.is_empty() {
        return Ok(None);
    }

    // All rows have the same task; take the first one by value, collect actions from the rest
    let mut iter = results.into_iter();
    let (base_task, first_action) = iter.next().unwrap();
    let actions: Vec<Action> = first_action
        .into_iter()
        .chain(iter.filter_map(|(_, a)| a))
        .collect();

    Ok(Some(TaskDto::new(base_task, actions)))
}

/// Atomically claim a Pending task by transitioning it to Claimed.
/// Returns true if this caller successfully claimed the task, false if another worker got it first.
pub async fn claim_task<'a>(conn: &mut Conn<'a>, task_id: &uuid::Uuid) -> Result<bool, DbError> {
    use crate::schema::task::dsl::*;
    use diesel::dsl::now;

    let updated_count =
        diesel::update(task.filter(id.eq(task_id).and(status.eq(StatusKind::Pending))))
            .set((status.eq(StatusKind::Claimed), last_updated.eq(now)))
            .execute(conn)
            .await?;

    Ok(updated_count == 1)
}

/// Transition a Claimed task to Running and set started_at.
/// Returns true if the task was updated, false if it was no longer Claimed.
pub async fn mark_task_running<'a>(
    conn: &mut Conn<'a>,
    task_id: &uuid::Uuid,
) -> Result<bool, DbError> {
    use crate::schema::task::dsl::*;
    use diesel::dsl::now;

    let updated_count =
        diesel::update(task.filter(id.eq(task_id).and(status.eq(StatusKind::Claimed))))
            .set((
                status.eq(StatusKind::Running),
                started_at.eq(now),
                last_updated.eq(now),
            ))
            .execute(conn)
            .await?;

    Ok(updated_count == 1)
}

/// Result of attempting to atomically check concurrency rules and claim a task.
#[derive(Debug, PartialEq)]
pub enum ClaimResult {
    /// Task was successfully claimed (Pending -> Claimed).
    Claimed,
    /// A concurrency rule blocked this task from being claimed.
    RuleBlocked,
    /// Task was already claimed by another worker (UPDATE touched 0 rows).
    AlreadyClaimed,
}

/// Compute a deterministic i64 advisory lock key from a concurrency rule and task metadata.
/// The key is derived from the rule's kind, status, and the task's metadata values for the
/// rule's fields.
pub(crate) fn concurrency_lock_key(
    rule: &crate::rule::ConcurencyRule,
    task_metadata: &serde_json::Value,
) -> i64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    rule.matcher.kind.hash(&mut hasher);
    rule.matcher.status.hash(&mut hasher);
    for field in &rule.matcher.fields {
        field.hash(&mut hasher);
        if let Some(val) = task_metadata.get(field) {
            val.to_string().hash(&mut hasher);
        }
    }
    hasher.finish() as i64
}

/// Compute a deterministic i64 advisory lock key for a capacity rule and task metadata.
/// Uses a "capacity" prefix to avoid collisions with concurrency lock keys.
pub(crate) fn capacity_lock_key(rule: &CapacityRule, task_metadata: &serde_json::Value) -> i64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    "capacity".hash(&mut hasher);
    rule.matcher.kind.hash(&mut hasher);
    rule.matcher.status.hash(&mut hasher);
    for field in &rule.matcher.fields {
        field.hash(&mut hasher);
        if let Some(val) = task_metadata.get(field) {
            val.to_string().hash(&mut hasher);
        }
    }
    hasher.finish() as i64
}

/// Atomically check concurrency rules and claim a task within a single transaction,
/// using `pg_advisory_xact_lock` to serialize workers checking the same rule/metadata combo.
///
/// This prevents the TOCTOU race where two workers both see count < max and both claim,
/// exceeding the concurrency limit.
///
/// Uses a two-query approach within the transaction:
/// 1. Acquire all advisory locks in one round-trip (via unnest)
/// 2. Check all concurrency + capacity rules and conditionally claim in a single CTE
///
/// This reduces the number of SQL round-trips from N+M+K+1 (N locks + M concurrency
/// checks + K capacity checks + 1 claim) to exactly 2, minimizing time spent holding
/// advisory locks and reducing contention between workers.
pub async fn claim_task_with_rules<'a>(
    conn: &mut Conn<'a>,
    t: &Task,
) -> Result<ClaimResult, DbError> {
    let rules = &t.start_condition.0;

    // No rules — just do a plain claim (no advisory lock needed)
    if rules.is_empty() {
        return match claim_task(conn, &t.id).await? {
            true => Ok(ClaimResult::Claimed),
            false => Ok(ClaimResult::AlreadyClaimed),
        };
    }

    // Pre-compute everything we need before entering the transaction closure.
    let task_id = t.id;

    let mut lock_keys: Vec<i64> = Vec::new();

    // Concurrency rule arrays (parallel arrays, one entry per rule)
    let mut conc_kinds: Vec<String> = Vec::new();
    let mut conc_meta_texts: Vec<String> = Vec::new();
    let mut conc_statuses: Vec<StatusKind> = Vec::new();
    let mut conc_include_claimed: Vec<bool> = Vec::new();
    let mut conc_thresholds: Vec<i64> = Vec::new();

    // Capacity rule arrays (parallel arrays, one entry per rule)
    let mut cap_kinds: Vec<String> = Vec::new();
    let mut cap_meta_texts: Vec<String> = Vec::new();
    let mut cap_max_capacities: Vec<i64> = Vec::new();

    for strategy in rules {
        match strategy {
            Strategy::Concurency(concurrency_rule) => {
                let mut m = json!({});
                let mut fields_ok = true;
                for field in &concurrency_rule.matcher.fields {
                    match t.metadata.get(field) {
                        Some(v) => {
                            m[field] = v.clone();
                        }
                        None => {
                            log::warn!(
                                "Task {} missing metadata field '{}' required by concurrency rule, blocking",
                                task_id,
                                field
                            );
                            fields_ok = false;
                            break;
                        }
                    }
                }
                if !fields_ok {
                    return Ok(ClaimResult::RuleBlocked);
                }

                let lock_key = concurrency_lock_key(concurrency_rule, &t.metadata);
                let is_same_kind = concurrency_rule.matcher.kind == t.kind;
                let include_claimed = concurrency_rule.matcher.status == StatusKind::Running;

                // Pre-compute threshold for the SQL check (`count >= threshold` means blocked):
                // is_same_kind  → count < max  → blocked when count >= max
                // !is_same_kind → count <= max → blocked when count >= max + 1
                let threshold = if is_same_kind {
                    concurrency_rule.max_concurency as i64
                } else {
                    (concurrency_rule.max_concurency + 1) as i64
                };

                lock_keys.push(lock_key);
                conc_kinds.push(concurrency_rule.matcher.kind.clone());
                conc_meta_texts.push(m.to_string());
                conc_statuses.push(concurrency_rule.matcher.status);
                conc_include_claimed.push(include_claimed);
                conc_thresholds.push(threshold);
            }
            Strategy::Capacity(capacity_rule) => {
                let mut m = json!({});
                let mut fields_ok = true;
                for field in &capacity_rule.matcher.fields {
                    match t.metadata.get(field) {
                        Some(v) => {
                            m[field] = v.clone();
                        }
                        None => {
                            log::warn!(
                                "Task {} missing metadata field '{}' required by capacity rule, blocking",
                                task_id,
                                field
                            );
                            fields_ok = false;
                            break;
                        }
                    }
                }
                if !fields_ok {
                    return Ok(ClaimResult::RuleBlocked);
                }

                // Candidate must have expected_count set
                if t.expected_count.is_none() {
                    log::warn!(
                        "Task {} has a Capacity rule but no expected_count, blocking",
                        task_id,
                    );
                    return Ok(ClaimResult::RuleBlocked);
                }

                let lock_key = capacity_lock_key(capacity_rule, &t.metadata);
                lock_keys.push(lock_key);
                cap_kinds.push(capacity_rule.matcher.kind.clone());
                cap_meta_texts.push(m.to_string());
                cap_max_capacities.push(capacity_rule.max_capacity as i64);
            }
        }
    }

    // Sort and deduplicate lock keys to acquire them in consistent order (prevents deadlocks)
    lock_keys.sort();
    lock_keys.dedup();

    run_in_transaction(conn, |conn| {
        Box::pin(async move {
            // Query 1: Acquire all advisory locks in one round-trip.
            // Locks are released automatically on COMMIT/ROLLBACK.
            diesel::sql_query(
                "SELECT pg_advisory_xact_lock(k) FROM unnest($1::bigint[]) AS k",
            )
            .bind::<diesel::sql_types::Array<diesel::sql_types::BigInt>, _>(&lock_keys)
            .execute(&mut *conn)
            .await?;

            // Query 2: Check all concurrency + capacity rules and conditionally claim
            // the task, all in a single CTE. Rule parameters are passed as parallel
            // arrays and unpacked via unnest.
            //
            // - conc_rules: one row per concurrency rule
            // - conc_blocked: rows where the concurrency count >= threshold
            // - cap_rules: one row per capacity rule
            // - cap_blocked: rows where the capacity sum >= max
            // - rules_check: single boolean — true iff no rule is blocked
            // - claim_result: conditional UPDATE, only executes if rules_check.ok is true
            #[derive(diesel::QueryableByName)]
            struct ClaimCheckRow {
                #[diesel(sql_type = diesel::sql_types::Bool)]
                rules_passed: bool,
                #[diesel(sql_type = diesel::sql_types::Bool)]
                claimed: bool,
            }

            // Note: meta_text values are produced by serde_json::Value::to_string(), which
            // always emits valid JSON. The SQL casts them back via `::jsonb`. This is safe but
            // less type-safe than the old code which passed metadata as Diesel's Jsonb type
            // directly — Diesel's sql_query bind API does not support binding Jsonb arrays, so
            // we pass them as text and cast in SQL.
            let row: ClaimCheckRow = diesel::sql_query(
                r#"
                WITH conc_rules AS (
                    SELECT ord, kind, meta_text, status_val, include_claimed, threshold
                    FROM unnest($1::text[], $2::text[], $3::status_kind[], $4::bool[], $5::bigint[])
                    WITH ORDINALITY AS r(kind, meta_text, status_val, include_claimed, threshold, ord)
                ),
                conc_blocked AS (
                    SELECT r.ord
                    FROM conc_rules r
                    WHERE (
                        SELECT COUNT(*)
                        FROM task t
                        WHERE t.kind = r.kind
                          AND t.metadata @> r.meta_text::jsonb
                          AND (
                              t.status = r.status_val
                              OR (r.include_claimed AND t.status = 'claimed')
                          )
                    ) >= r.threshold
                ),
                cap_rules AS (
                    SELECT ord, kind, meta_text, max_cap
                    FROM unnest($6::text[], $7::text[], $8::bigint[])
                    WITH ORDINALITY AS r(kind, meta_text, max_cap, ord)
                ),
                cap_blocked AS (
                    SELECT r.ord
                    FROM cap_rules r
                    WHERE (
                        SELECT COALESCE(SUM(GREATEST(COALESCE(t.expected_count, 0) - t.success - t.failures, 0)), 0)
                        FROM task t
                        WHERE t.kind = r.kind
                          AND (t.status = 'running' OR t.status = 'claimed')
                          AND t.metadata @> r.meta_text::jsonb
                    ) >= r.max_cap
                ),
                rules_check AS (
                    SELECT
                        NOT EXISTS (SELECT 1 FROM conc_blocked)
                        AND NOT EXISTS (SELECT 1 FROM cap_blocked) AS ok
                ),
                claim_result AS (
                    UPDATE task SET status = 'claimed', last_updated = now()
                    WHERE id = $9 AND status = 'pending'
                      AND (SELECT ok FROM rules_check)
                    RETURNING id
                )
                SELECT
                    (SELECT ok FROM rules_check) AS rules_passed,
                    EXISTS (SELECT 1 FROM claim_result) AS claimed
                "#,
            )
            .bind::<diesel::sql_types::Array<diesel::sql_types::Text>, _>(&conc_kinds)
            .bind::<diesel::sql_types::Array<diesel::sql_types::Text>, _>(&conc_meta_texts)
            .bind::<diesel::sql_types::Array<crate::schema::sql_types::StatusKind>, _>(&conc_statuses)
            .bind::<diesel::sql_types::Array<diesel::sql_types::Bool>, _>(&conc_include_claimed)
            .bind::<diesel::sql_types::Array<diesel::sql_types::BigInt>, _>(&conc_thresholds)
            .bind::<diesel::sql_types::Array<diesel::sql_types::Text>, _>(&cap_kinds)
            .bind::<diesel::sql_types::Array<diesel::sql_types::Text>, _>(&cap_meta_texts)
            .bind::<diesel::sql_types::Array<diesel::sql_types::BigInt>, _>(&cap_max_capacities)
            .bind::<diesel::sql_types::Uuid, _>(task_id)
            // INVARIANT: The final SELECT (no FROM/WHERE) always produces exactly 1 row,
            // so get_result is safe. If this query is ever modified to add filtering on the
            // outer SELECT, it must switch to get_results + handle the empty case.
            .get_result(&mut *conn)
            .await?;

            if row.claimed {
                Ok(ClaimResult::Claimed)
            } else if !row.rules_passed {
                Ok(ClaimResult::RuleBlocked)
            } else {
                Ok(ClaimResult::AlreadyClaimed)
            }
        })
    })
    .await
}
