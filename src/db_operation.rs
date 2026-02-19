use crate::{
    Conn,
    action::ActionExecutor,
    dtos::{self, TaskDto},
    metrics,
    models::{self, Action, Link, NewAction, StatusKind, Task},
    rule::{Matcher, Strategy},
};
use chrono::Utc;
use diesel::prelude::*;
use diesel::sql_types;
use diesel_async::RunQueryDsl;
use serde_json::json;
use std::collections::HashMap;
use std::future::Future;
use std::hash::{Hash, Hasher};
use std::pin::Pin;
use uuid::Uuid;
pub(crate) type DbError = Box<dyn std::error::Error + Send + Sync>;

/// Escape special LIKE pattern characters (`%`, `_`, `\`) in user input
/// so they are matched literally.
use crate::dtos::escape_like_pattern;

/// Execute a closure within a database transaction.
/// Automatically rolls back on error. Commits on success.
/// Callers must wrap their async block with `Box::pin(async move { ... })`.
pub async fn run_in_transaction<'a, T: Send>(
    conn: &mut Conn<'a>,
    f: impl for<'c> FnOnce(
        &'c mut Conn<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<T, DbError>> + Send + 'c>>,
) -> Result<T, DbError> {
    diesel::sql_query("BEGIN").execute(&mut *conn).await?;
    match f(conn).await {
        Ok(val) => {
            diesel::sql_query("COMMIT").execute(&mut *conn).await?;
            Ok(val)
        }
        Err(e) => {
            if let Err(rb_err) = diesel::sql_query("ROLLBACK").execute(&mut *conn).await {
                log::error!("Failed to rollback transaction: {}", rb_err);
            }
            Err(e)
        }
    }
}

/// TaskDto is a data transfer object that represents a task with its actions.
impl TaskDto {
    pub fn new(base_task: Task, actions: Vec<Action>) -> Self {
        Self {
            id: base_task.id,
            name: base_task.name,
            kind: base_task.kind,
            status: base_task.status,
            timeout: base_task.timeout,
            rules: base_task.start_condition,
            metadata: base_task.metadata,
            created_at: base_task.created_at,
            success: base_task.success,
            failures: base_task.failures,
            ended_at: base_task.ended_at,
            last_updated: base_task.last_updated,
            started_at: base_task.started_at,
            failure_reason: base_task.failure_reason,
            batch_id: base_task.batch_id,
            actions: actions
                .into_iter()
                .map(|a| dtos::ActionDto {
                    kind: a.kind,
                    params: a.params,
                    trigger: a.trigger,
                })
                .collect(),
        }
    }
}

/// Update all tasks with status running and started_at older than timeout to failed and update
/// the ended_at field to the current time.
pub(crate) async fn ensure_pending_tasks_timeout<'a>(
    conn: &mut Conn<'a>,
) -> Result<Vec<Task>, DbError> {
    use {
        crate::schema::task::dsl::*,
        diesel::{dsl::now, pg::data_types::PgInterval},
    };
    const TIMEOUT_REASON: &str = "Timeout";
    let updated = diesel::update(
        task.filter(
            status
                .eq(models::StatusKind::Running)
                .and(started_at.is_not_null())
                .and(last_updated.lt(now.into_sql::<sql_types::Timestamptz>()
                    - (PgInterval::from_microseconds(1_000_000).into_sql::<sql_types::Interval>()
                        * timeout))),
        ),
    )
    .set((
        status.eq(models::StatusKind::Failure),
        ended_at.eq(now),
        failure_reason.eq(TIMEOUT_REASON),
    ))
    .returning(Task::as_returning())
    .get_results::<Task>(conn)
    .await?;
    Ok(updated)
}

/// Requeue Claimed tasks that never started within the claim timeout.
/// Returns the tasks moved back to Pending.
pub(crate) async fn requeue_stale_claimed_tasks<'a>(
    conn: &mut Conn<'a>,
    claim_timeout: std::time::Duration,
) -> Result<Vec<Task>, DbError> {
    use {
        crate::schema::task::dsl::*,
        diesel::{dsl::now, pg::data_types::PgInterval},
    };

    let micros = i64::try_from(claim_timeout.as_micros()).unwrap_or(i64::MAX);
    let interval = PgInterval::from_microseconds(micros).into_sql::<sql_types::Interval>();

    let updated = diesel::update(
        task.filter(
            status
                .eq(models::StatusKind::Claimed)
                .and(last_updated.lt(now.into_sql::<sql_types::Timestamptz>() - interval)),
        ),
    )
    .set((status.eq(models::StatusKind::Pending), last_updated.eq(now)))
    .returning(Task::as_returning())
    .get_results::<Task>(conn)
    .await?;

    Ok(updated)
}

pub(crate) async fn list_all_pending<'a>(conn: &mut Conn<'a>) -> Result<Vec<Task>, DbError> {
    use crate::schema::task::dsl::*;
    let tasks = task
        .filter(status.eq(models::StatusKind::Pending))
        .order(created_at.asc())
        .get_results(conn)
        .await?;
    Ok(tasks)
}
/// Result of attempting to update a running task.
#[derive(Debug, PartialEq)]
pub enum UpdateTaskResult {
    /// Task was successfully updated (transitioned from Running).
    Updated,
    /// Task does not exist or was not in Running state.
    NotFound,
}

#[tracing::instrument(name = "update_running_task", skip(evaluator, conn, dto), fields(task_id = %task_id))]
pub async fn update_running_task<'a>(
    evaluator: &ActionExecutor,
    conn: &mut Conn<'a>,
    task_id: Uuid,
    dto: dtos::UpdateTaskDto,
) -> Result<UpdateTaskResult, DbError> {
    use crate::schema::task::dsl::*;
    use crate::workers;
    use tracing::Instrument;
    let s = dto.status;
    let final_status_clone = dto.status;

    let has_status_change = dto.status.is_some();

    let res = if has_status_change {
        // Status change: transaction needed for atomic UPDATE + propagation
        run_in_transaction(conn, |conn| {
            Box::pin(async move {
                let res = diesel::update(
                    task.filter(id.eq(task_id).and(status.eq(models::StatusKind::Running))),
                )
                .set((
                    last_updated.eq(diesel::dsl::now),
                    dto.new_success.map(|e| success.eq(success + e)),
                    dto.new_failures.map(|e| failures.eq(failures + e)),
                    s.filter(|e| {
                        e == &models::StatusKind::Success || e == &models::StatusKind::Failure
                    })
                    .map(|_| ended_at.eq(diesel::dsl::now)),
                    dto.metadata.map(|m| metadata.eq(m)),
                    dto.status.as_ref().map(|m| status.eq(m)),
                    dto.failure_reason.map(|m| failure_reason.eq(m)),
                ))
                .execute(conn)
                .await?;

                if res == 1 {
                    if let Some(ref final_status) = dto.status {
                        workers::propagate_to_children(&task_id, final_status, conn).await?;
                    }
                }

                Ok(res)
            })
        })
        .instrument(tracing::info_span!("tx_update_and_propagate"))
        .await?
    } else {
        // Counter-only update: no transaction needed, autocommit for minimal row lock
        diesel::update(task.filter(id.eq(task_id).and(status.eq(models::StatusKind::Running))))
            .set((
                last_updated.eq(diesel::dsl::now),
                dto.new_success.map(|e| success.eq(success + e)),
                dto.new_failures.map(|e| failures.eq(failures + e)),
                dto.metadata.map(|m| metadata.eq(m)),
            ))
            .execute(conn)
            .await?
    };

    // After commit: fire webhooks and record metrics (best-effort, outside transaction)
    if let Some(ref final_status) = final_status_clone {
        if res == 1 {
            let outcome = match final_status {
                models::StatusKind::Success => "success",
                models::StatusKind::Failure => "failure",
                _ => "other",
            };
            metrics::record_status_transition("Running", outcome);
            match workers::fire_end_webhooks(evaluator, &task_id, *final_status, conn)
                .instrument(tracing::info_span!("fire_end_webhooks"))
                .await
            {
                Ok(_) => log::debug!("task {} end webhooks fired successfully", &task_id),
                Err(e) => log::error!("task {} end webhooks failed: {}", &task_id, e),
            }
        } else {
            log::warn!(
                "update_running_task: task {} was not in Running state, skipping end_task",
                task_id
            );
        }
    }

    if res == 1 {
        Ok(UpdateTaskResult::Updated)
    } else {
        Ok(UpdateTaskResult::NotFound)
    }
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

pub(crate) async fn list_task_filtered_paged<'a>(
    conn: &mut Conn<'a>,
    pagination: dtos::Pagination,
    filter: dtos::Filter,
) -> Result<Vec<dtos::BasicTaskDto>, DbError> {
    use crate::schema::task::dsl::*;
    use diesel::PgJsonbExpressionMethods;

    let mut query = task
        .into_boxed()
        .offset(pagination.offset)
        .limit(pagination.limit)
        .order(created_at.desc())
        .filter(name.like(format!("%{}%", filter.name)))
        .filter(kind.like(format!("%{}%", filter.kind)));

    if let Some(val) = filter.metadata {
        query = query.filter(metadata.contains(val));
    }

    if let Some(s) = filter.status {
        query = query.filter(status.eq(s));
    }

    if let Some(bid) = filter.batch_id {
        query = query.filter(batch_id.eq(bid));
    }

    if let Some(t) = filter.timeout {
        query = query.filter(timeout.eq(t));
    }

    let result = query.load::<models::Task>(conn).await?;

    let tasks: Vec<dtos::BasicTaskDto> = result
        .into_iter()
        .map(|base_task| dtos::BasicTaskDto {
            id: base_task.id,
            name: base_task.name,
            kind: base_task.kind,
            status: base_task.status,
            created_at: base_task.created_at,
            ended_at: base_task.ended_at,
            started_at: base_task.started_at,
            success: base_task.success,
            failures: base_task.failures,
            batch_id: base_task.batch_id,
        })
        .collect();

    Ok(tasks)
}

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

async fn insert_actions<'a>(
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

/// Atomically check concurrency rules and claim a task within a single transaction,
/// using `pg_advisory_xact_lock` to serialize workers checking the same rule/metadata combo.
///
/// This prevents the TOCTOU race where two workers both see count < max and both claim,
/// exceeding the concurrency limit.
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
    // Build lock keys and rule checks from &t references to avoid cloning task_metadata.
    let task_id = t.id;

    let mut lock_keys = Vec::new();
    let mut rule_checks: Vec<(crate::rule::ConcurencyRule, serde_json::Value, bool)> = Vec::new();

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
                lock_keys.push(lock_key);
                rule_checks.push((concurrency_rule.clone(), m, is_same_kind));
            }
        }
    }

    // Sort and deduplicate lock keys to acquire them in consistent order (prevents deadlocks)
    lock_keys.sort();
    lock_keys.dedup();

    run_in_transaction(conn, |conn| {
        Box::pin(async move {
            // Acquire all advisory locks (released automatically on COMMIT/ROLLBACK)
            for key in &lock_keys {
                diesel::sql_query(format!("SELECT pg_advisory_xact_lock({})", key))
                    .execute(&mut *conn)
                    .await?;
            }

            // Check all concurrency rules
            for (concurrency_rule, metadata_filter, is_same_kind) in &rule_checks {
                use crate::schema::task::dsl;
                use diesel::PgJsonbExpressionMethods;

                let mut query = dsl::task
                    .into_boxed()
                    .filter(dsl::kind.eq(&concurrency_rule.matcher.kind))
                    .filter(dsl::metadata.contains(metadata_filter.clone()));

                if concurrency_rule.matcher.status == StatusKind::Running {
                    query = query.filter(
                        dsl::status
                            .eq(StatusKind::Running)
                            .or(dsl::status.eq(StatusKind::Claimed)),
                    );
                } else {
                    query = query.filter(dsl::status.eq(&concurrency_rule.matcher.status));
                }

                let count = query.count().get_result::<i64>(&mut *conn).await?;

                let allowed = if *is_same_kind {
                    count < concurrency_rule.max_concurency.into()
                } else {
                    count <= concurrency_rule.max_concurency.into()
                };

                if !allowed {
                    return Ok(ClaimResult::RuleBlocked);
                }
            }

            // All rules passed — claim the task
            use crate::schema::task::dsl;
            use diesel::dsl::now;

            let updated_count = diesel::update(
                dsl::task.filter(dsl::id.eq(task_id).and(dsl::status.eq(StatusKind::Pending))),
            )
            .set((
                dsl::status.eq(StatusKind::Claimed),
                dsl::last_updated.eq(now),
            ))
            .execute(&mut *conn)
            .await?;

            if updated_count == 1 {
                Ok(ClaimResult::Claimed)
            } else {
                Ok(ClaimResult::AlreadyClaimed)
            }
        })
    })
    .await
}

/// Save cancel actions for a task that has been claimed.
/// Validates webhook URLs before saving to prevent SSRF via cancel action responses.
pub(crate) async fn save_cancel_actions<'a>(
    conn: &mut Conn<'a>,
    t: &Task,
    cancel_tasks: &[dtos::NewActionDto],
) -> Result<(), DbError> {
    if !cancel_tasks.is_empty() {
        // Validate each cancel action's webhook URL before persisting
        for action_dto in cancel_tasks {
            if let Err(e) =
                crate::validation::validate_action_params(&action_dto.kind, &action_dto.params)
            {
                log::warn!(
                    "Rejecting cancel action for task {} due to validation failure: {}",
                    t.id,
                    e
                );
                return Err(Box::from(format!("Cancel action validation failed: {}", e)));
            }
        }
        insert_actions(
            t.id,
            cancel_tasks,
            &models::TriggerKind::Cancel,
            &models::TriggerCondition::Success, // Condition doesn't matter for cancel
            conn,
        )
        .await?;
    }
    Ok(())
}

/// Mark a task as failed with a reason. Returns true if the task was updated.
/// Used when on_start webhook fails after claim.
pub(crate) async fn mark_task_failed<'a>(
    conn: &mut Conn<'a>,
    task_id: &uuid::Uuid,
    reason: &str,
) -> Result<bool, DbError> {
    use crate::schema::task::dsl::*;
    use diesel::dsl::now;

    let updated = diesel::update(
        task.filter(
            id.eq(task_id).and(
                status
                    .eq(StatusKind::Running)
                    .or(status.eq(StatusKind::Claimed)),
            ),
        ),
    )
    .set((
        status.eq(StatusKind::Failure),
        failure_reason.eq(reason),
        ended_at.eq(now),
        last_updated.eq(now),
    ))
    .execute(conn)
    .await?;
    Ok(updated == 1)
}

/// Mark a task as failed, propagate failure to children (in a transaction),
/// then fire on_failure webhooks (best-effort, outside transaction).
/// Used when on_start webhook fails after claim.
pub(crate) async fn fail_task_and_propagate<'a>(
    evaluator: &ActionExecutor,
    conn: &mut Conn<'a>,
    task_id: &uuid::Uuid,
    reason: &str,
) -> Result<(), DbError> {
    use crate::workers;

    // Wrap status update + propagation in a transaction
    let tid = *task_id;
    let reason_owned = reason.to_string();
    let updated = run_in_transaction(conn, |conn| {
        Box::pin(async move {
            let updated = mark_task_failed(conn, &tid, &reason_owned).await?;
            if updated {
                workers::propagate_to_children(&tid, &StatusKind::Failure, conn).await?;
            }
            Ok(updated)
        })
    })
    .await?;

    if !updated {
        log::warn!(
            "fail_task_and_propagate: task {} not in Running/Claimed state; skipping failure propagation",
            task_id
        );
        return Ok(());
    }

    // After commit: fire on_failure webhooks (best-effort)
    match workers::fire_end_webhooks(evaluator, task_id, StatusKind::Failure, conn).await {
        Ok(_) => log::debug!(
            "task {} on_failure webhooks fired after on_start failure",
            task_id
        ),
        Err(e) => log::error!("task {} on_failure webhooks failed: {}", task_id, e),
    }

    Ok(())
}

pub(crate) async fn pause_task<'a>(
    task_id: &uuid::Uuid,
    conn: &mut Conn<'a>,
) -> Result<(), DbError> {
    use crate::schema::task::dsl::{id, status, task};
    let updated = diesel::update(
        task.filter(
            id.eq(task_id).and(
                status
                    .eq(StatusKind::Pending)
                    .or(status.eq(StatusKind::Claimed))
                    .or(status.eq(StatusKind::Running))
                    .or(status.eq(StatusKind::Waiting)),
            ),
        ),
    )
    .set(status.eq(StatusKind::Paused))
    .execute(conn)
    .await?;
    if updated == 0 {
        return Err(Box::from(
            "Invalid operation: cannot pause task in this state",
        ));
    }
    Ok(())
}

/// Get DAG data for a batch: all tasks and their links
pub(crate) async fn get_dag_for_batch<'a>(
    conn: &mut Conn<'a>,
    bid: Uuid,
) -> Result<dtos::DagDto, DbError> {
    use crate::schema::link::dsl::link;
    use crate::schema::task::dsl::*;

    // Get all tasks in the batch
    let tasks_result = task
        .filter(batch_id.eq(bid))
        .order(created_at.asc())
        .load::<models::Task>(conn)
        .await?;

    let task_ids: Vec<Uuid> = tasks_result.iter().map(|t| t.id).collect();

    // Get all links where both parent and child are in this batch
    let links_result = link
        .filter(
            crate::schema::link::dsl::parent_id
                .eq_any(&task_ids)
                .and(crate::schema::link::dsl::child_id.eq_any(&task_ids)),
        )
        .load::<Link>(conn)
        .await?;

    let tasks_dto: Vec<dtos::BasicTaskDto> = tasks_result
        .into_iter()
        .map(|t| dtos::BasicTaskDto {
            id: t.id,
            name: t.name,
            kind: t.kind,
            status: t.status,
            created_at: t.created_at,
            ended_at: t.ended_at,
            started_at: t.started_at,
            success: t.success,
            failures: t.failures,
            batch_id: t.batch_id,
        })
        .collect();

    let links_dto: Vec<dtos::LinkDto> = links_result
        .into_iter()
        .map(|l| dtos::LinkDto {
            parent_id: l.parent_id,
            child_id: l.child_id,
            requires_success: l.requires_success,
        })
        .collect();

    Ok(dtos::DagDto {
        tasks: tasks_dto,
        links: links_dto,
    })
}

// =============================================================================
// Batch Listing
// =============================================================================

/// Intermediate row for mapping `list_batches` SQL query results.
#[derive(Debug, diesel::QueryableByName)]
struct BatchSummaryRow {
    #[diesel(sql_type = sql_types::Uuid)]
    batch_id: uuid::Uuid,
    #[diesel(sql_type = sql_types::BigInt)]
    total_tasks: i64,
    #[diesel(sql_type = sql_types::Timestamptz)]
    first_created_at: chrono::DateTime<Utc>,
    #[diesel(sql_type = sql_types::Timestamptz)]
    latest_updated_at: chrono::DateTime<Utc>,
    #[diesel(sql_type = sql_types::BigInt)]
    waiting: i64,
    #[diesel(sql_type = sql_types::BigInt)]
    pending: i64,
    #[diesel(sql_type = sql_types::BigInt)]
    claimed: i64,
    #[diesel(sql_type = sql_types::BigInt)]
    running: i64,
    #[diesel(sql_type = sql_types::BigInt)]
    success: i64,
    #[diesel(sql_type = sql_types::BigInt)]
    failure: i64,
    #[diesel(sql_type = sql_types::BigInt)]
    paused: i64,
    #[diesel(sql_type = sql_types::BigInt)]
    canceled: i64,
    #[diesel(sql_type = sql_types::Array<sql_types::Text>)]
    kinds: Vec<String>,
}

/// List batches with aggregated statistics, supporting optional filters and pagination.
pub(crate) async fn list_batches<'a>(
    conn: &mut Conn<'a>,
    pagination: dtos::Pagination,
    filter: dtos::BatchFilterDto,
) -> Result<Vec<dtos::BatchSummaryDto>, DbError> {
    // Build CTE with dynamic WHERE clauses.
    // String filters are safely escaped and inlined; timestamps use bind params.
    let mut cte_conditions = vec!["batch_id IS NOT NULL".to_string()];
    let mut ts_binds: Vec<chrono::DateTime<Utc>> = vec![];
    let mut next_param = 1usize; // $1, $2... reserved for timestamps, then limit/offset

    if let Some(ref kind_filter) = filter.kind {
        let escaped = escape_like_pattern(kind_filter).replace('\'', "''");
        cte_conditions.push(format!("kind LIKE '%{}%'", escaped));
    }

    if let Some(ref status_filter) = filter.status {
        let status_str = match status_filter {
            models::StatusKind::Waiting => "waiting",
            models::StatusKind::Pending => "pending",
            models::StatusKind::Claimed => "claimed",
            models::StatusKind::Running => "running",
            models::StatusKind::Success => "success",
            models::StatusKind::Failure => "failure",
            models::StatusKind::Paused => "paused",
            models::StatusKind::Canceled => "canceled",
        };
        cte_conditions.push(format!("status = '{}'::status_kind", status_str));
    }

    if let Some(ref name_filter) = filter.name {
        let escaped = escape_like_pattern(name_filter).replace('\'', "''");
        cte_conditions.push(format!("name LIKE '%{}%'", escaped));
    }

    if let Some(ref _created_after) = filter.created_after {
        ts_binds.push(*_created_after);
        cte_conditions.push(format!("created_at >= ${}", next_param));
        next_param += 1;
    }

    if let Some(ref _created_before) = filter.created_before {
        ts_binds.push(*_created_before);
        cte_conditions.push(format!("created_at <= ${}", next_param));
        next_param += 1;
    }

    let limit_param = next_param;
    next_param += 1;
    let offset_param = next_param;

    let where_clause = cte_conditions.join(" AND ");

    let sql = format!(
        r#"
        WITH qualifying_batches AS (
            SELECT DISTINCT batch_id FROM task
            WHERE {where_clause}
        )
        SELECT
            t.batch_id,
            COUNT(*)::bigint AS total_tasks,
            MIN(t.created_at) AS first_created_at,
            MAX(t.last_updated) AS latest_updated_at,
            COUNT(*) FILTER (WHERE t.status = 'waiting')::bigint AS waiting,
            COUNT(*) FILTER (WHERE t.status = 'pending')::bigint AS pending,
            COUNT(*) FILTER (WHERE t.status = 'claimed')::bigint AS claimed,
            COUNT(*) FILTER (WHERE t.status = 'running')::bigint AS running,
            COUNT(*) FILTER (WHERE t.status = 'success')::bigint AS success,
            COUNT(*) FILTER (WHERE t.status = 'failure')::bigint AS failure,
            COUNT(*) FILTER (WHERE t.status = 'paused')::bigint AS paused,
            COUNT(*) FILTER (WHERE t.status = 'canceled')::bigint AS canceled,
            ARRAY_AGG(DISTINCT t.kind) AS kinds
        FROM task t
        INNER JOIN qualifying_batches qb ON t.batch_id = qb.batch_id
        GROUP BY t.batch_id
        ORDER BY MIN(t.created_at) DESC
        LIMIT ${limit_param} OFFSET ${offset_param}
        "#
    );

    // Chain .bind() calls for timestamps, then limit/offset
    // We use a helper that accepts up to N timestamp binds + limit + offset.
    let rows =
        list_batches_execute(conn, &sql, &ts_binds, pagination.limit, pagination.offset).await?;

    let results = rows
        .into_iter()
        .map(|r| dtos::BatchSummaryDto {
            batch_id: r.batch_id,
            total_tasks: r.total_tasks,
            first_created_at: r.first_created_at,
            latest_updated_at: r.latest_updated_at,
            status_counts: dtos::BatchStatusCounts {
                waiting: r.waiting,
                pending: r.pending,
                claimed: r.claimed,
                running: r.running,
                success: r.success,
                failure: r.failure,
                paused: r.paused,
                canceled: r.canceled,
            },
            kinds: r.kinds,
        })
        .collect();

    Ok(results)
}

/// Helper enum for bind value types (unused — kept for reference).
#[allow(dead_code)]
enum BindValue {
    String(String),
    Timestamp(chrono::DateTime<Utc>),
    BigInt(i64),
}

/// Execute the list_batches SQL with the appropriate number of timestamp binds.
/// Diesel's `.bind()` returns a new type each time, so we handle 0/1/2 timestamp
/// cases explicitly.
async fn list_batches_execute<'a>(
    conn: &mut Conn<'a>,
    sql: &str,
    ts_binds: &[chrono::DateTime<Utc>],
    limit: i64,
    offset: i64,
) -> Result<Vec<BatchSummaryRow>, DbError> {
    match ts_binds.len() {
        0 => {
            let rows = diesel::sql_query(sql)
                .bind::<sql_types::BigInt, _>(limit)
                .bind::<sql_types::BigInt, _>(offset)
                .get_results::<BatchSummaryRow>(conn)
                .await?;
            Ok(rows)
        }
        1 => {
            let rows = diesel::sql_query(sql)
                .bind::<sql_types::Timestamptz, _>(ts_binds[0])
                .bind::<sql_types::BigInt, _>(limit)
                .bind::<sql_types::BigInt, _>(offset)
                .get_results::<BatchSummaryRow>(conn)
                .await?;
            Ok(rows)
        }
        2 => {
            let rows = diesel::sql_query(sql)
                .bind::<sql_types::Timestamptz, _>(ts_binds[0])
                .bind::<sql_types::Timestamptz, _>(ts_binds[1])
                .bind::<sql_types::BigInt, _>(limit)
                .bind::<sql_types::BigInt, _>(offset)
                .get_results::<BatchSummaryRow>(conn)
                .await?;
            Ok(rows)
        }
        _ => Err(Box::from("too many timestamp filters")),
    }
}

/// Delete terminal tasks (Success/Failure/Canceled) with `ended_at` older than the
/// retention period. Deletes in FK order (actions → links → tasks) within a transaction.
/// Returns the number of tasks deleted.
pub(crate) async fn cleanup_old_terminal_tasks<'a>(
    conn: &mut Conn<'a>,
    retention_days: u32,
    batch_size: i64,
) -> Result<usize, DbError> {
    use crate::schema::task::dsl as task_dsl;

    let cutoff = Utc::now() - chrono::Duration::days(retention_days as i64);

    // Find task IDs eligible for cleanup
    let task_ids: Vec<uuid::Uuid> = task_dsl::task
        .filter(
            task_dsl::status
                .eq(StatusKind::Success)
                .or(task_dsl::status.eq(StatusKind::Failure))
                .or(task_dsl::status.eq(StatusKind::Canceled)),
        )
        .filter(task_dsl::ended_at.le(cutoff))
        .select(task_dsl::id)
        .limit(batch_size)
        .load::<uuid::Uuid>(conn)
        .await?;

    if task_ids.is_empty() {
        return Ok(0);
    }

    let count = task_ids.len();

    run_in_transaction(conn, |conn| {
        Box::pin(async move {
            use crate::schema::action::dsl as action_dsl;
            use crate::schema::link::dsl as link_dsl;

            // 1. Delete actions belonging to these tasks
            diesel::delete(action_dsl::action.filter(action_dsl::task_id.eq_any(&task_ids)))
                .execute(&mut *conn)
                .await?;

            // 2. Delete links referencing these tasks (as parent or child)
            diesel::delete(
                link_dsl::link.filter(
                    link_dsl::parent_id
                        .eq_any(&task_ids)
                        .or(link_dsl::child_id.eq_any(&task_ids)),
                ),
            )
            .execute(&mut *conn)
            .await?;

            // 3. Delete the tasks themselves
            diesel::delete(task_dsl::task.filter(task_dsl::id.eq_any(&task_ids)))
                .execute(&mut *conn)
                .await?;

            Ok(count)
        })
    })
    .await
}
