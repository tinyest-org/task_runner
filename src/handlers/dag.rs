use actix_web::http::header::{CacheControl, CacheDirective, ContentType};
use actix_web::{HttpResponse, web};
use uuid::Uuid;

use crate::{db_operation, dtos, error::ApiError};

use super::AppState;

#[utoipa::path(
    get,
    path = "/dag/{batch_id}",
    summary = "Get DAG structure",
    description = "Returns all tasks and dependency links for a given batch. Use this to visualize or inspect the full dependency graph. The `batch_id` is returned in the `X-Batch-ID` header of `POST /task` responses.",
    params(("batch_id" = Uuid, Path, description = "The batch UUID (from the X-Batch-ID response header of POST /task)")),
    responses(
        (status = 200, description = "DAG structure with tasks and links", body = dtos::DagDto),
    ),
    tag = "dag"
)]
/// Get DAG data for a batch (tasks + links)
pub async fn get_dag(
    state: web::Data<AppState>,
    batch_id: web::Path<Uuid>,
) -> actix_web::Result<HttpResponse> {
    let mut conn = state.conn().await?;

    let dag = db_operation::get_dag_for_batch(&mut conn, *batch_id)
        .await
        .map_err(ApiError::from)?;

    Ok(HttpResponse::Ok().json(dag))
}

#[utoipa::path(
    get,
    path = "/view",
    summary = "DAG visualization UI",
    description = "Serves an HTML page with an interactive DAG visualizer. Open this in a browser and provide a `batch_id` to render the task graph. This is a human-facing UI, not an API endpoint.",
    responses(
        (status = 200, description = "HTML page for DAG visualization", content_type = "text/html"),
    ),
    tag = "dag"
)]
/// Serve the DAG visualization HTML page
pub async fn view_dag_page() -> HttpResponse {
    HttpResponse::Ok()
        .content_type(ContentType::html())
        .insert_header(CacheControl(vec![CacheDirective::NoCache]))
        .body(include_str!("../../static/dag.html"))
}
