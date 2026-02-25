use crate::{
    Conn,
    dtos::{self, escape_like_pattern},
    models,
};
use chrono::Utc;
use diesel::prelude::*;
use diesel::sql_types;
use diesel_async::RunQueryDsl;

use super::DbError;

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
        _ => Err(crate::error::TaskRunnerError::Internal(
            "too many timestamp filters".into(),
        )),
    }
}

/// Get aggregated stats (success, failures, expected_count) for a single batch.
/// Uses a single GROUP BY query — returns None if no tasks match the batch_id.
pub(crate) async fn get_batch_stats<'a>(
    conn: &mut Conn<'a>,
    bid: uuid::Uuid,
) -> Result<Option<dtos::BatchStatsDto>, DbError> {
    #[derive(Debug, diesel::QueryableByName)]
    struct BatchStatsRow {
        #[diesel(sql_type = sql_types::BigInt)]
        total_tasks: i64,
        #[diesel(sql_type = sql_types::BigInt)]
        total_success: i64,
        #[diesel(sql_type = sql_types::BigInt)]
        total_failures: i64,
        #[diesel(sql_type = sql_types::Nullable<sql_types::BigInt>)]
        total_expected: Option<i64>,
        #[diesel(sql_type = sql_types::BigInt)]
        waiting: i64,
        #[diesel(sql_type = sql_types::BigInt)]
        pending: i64,
        #[diesel(sql_type = sql_types::BigInt)]
        claimed: i64,
        #[diesel(sql_type = sql_types::BigInt)]
        running: i64,
        #[diesel(sql_type = sql_types::BigInt)]
        cnt_success: i64,
        #[diesel(sql_type = sql_types::BigInt)]
        cnt_failure: i64,
        #[diesel(sql_type = sql_types::BigInt)]
        paused: i64,
        #[diesel(sql_type = sql_types::BigInt)]
        canceled: i64,
    }

    let row = diesel::sql_query(
        r#"
        SELECT
            COUNT(*)::bigint AS total_tasks,
            COALESCE(SUM(success), 0)::bigint AS total_success,
            COALESCE(SUM(failures), 0)::bigint AS total_failures,
            CASE WHEN COUNT(*) FILTER (WHERE expected_count IS NULL) = 0
                 THEN SUM(expected_count)::bigint
            END AS total_expected,
            COUNT(*) FILTER (WHERE status = 'waiting')::bigint AS waiting,
            COUNT(*) FILTER (WHERE status = 'pending')::bigint AS pending,
            COUNT(*) FILTER (WHERE status = 'claimed')::bigint AS claimed,
            COUNT(*) FILTER (WHERE status = 'running')::bigint AS running,
            COUNT(*) FILTER (WHERE status = 'success')::bigint AS cnt_success,
            COUNT(*) FILTER (WHERE status = 'failure')::bigint AS cnt_failure,
            COUNT(*) FILTER (WHERE status = 'paused')::bigint AS paused,
            COUNT(*) FILTER (WHERE status = 'canceled')::bigint AS canceled
        FROM task
        WHERE batch_id = $1
        GROUP BY batch_id
        "#,
    )
    .bind::<sql_types::Uuid, _>(bid)
    .get_result::<BatchStatsRow>(conn)
    .await
    .optional()?;

    Ok(row.map(|r| dtos::BatchStatsDto {
        batch_id: bid,
        total_tasks: r.total_tasks,
        total_success: r.total_success,
        total_failures: r.total_failures,
        total_expected: r.total_expected,
        status_counts: dtos::BatchStatusCounts {
            waiting: r.waiting,
            pending: r.pending,
            claimed: r.claimed,
            running: r.running,
            success: r.cnt_success,
            failure: r.cnt_failure,
            paused: r.paused,
            canceled: r.canceled,
        },
    }))
}
