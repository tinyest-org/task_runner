use actix_http::Request;
use actix_web::body::MessageBody;
use actix_web::dev::{Service, ServiceResponse};
use task_runner::dtos::TaskDto;
use task_runner::handlers::AppState;
use task_runner::models::StatusKind;

use super::setup::{TestApp, setup_test_db};
use super::state::{
    TestStateWithBatchUpdater, create_test_state, create_test_state_with_batch_updater,
};

/// Setup a complete test app: database + state. Returns (TestApp guard, AppState).
/// The TestApp must be kept alive (_g pattern) to keep the container running.
pub async fn setup_test_app() -> (TestApp, AppState) {
    let test_app = setup_test_db().await;
    let state = create_test_state(test_app.pool.clone());
    (test_app, state)
}

/// Setup a test app with a running batch updater.
pub async fn setup_test_app_with_batch_updater() -> (TestApp, TestStateWithBatchUpdater) {
    let test_app = setup_test_db().await;
    let test_state = create_test_state_with_batch_updater(test_app.pool.clone());
    (test_app, test_state)
}

/// POST /task with given tasks, assert 201, return deserialized Vec<TaskDto>.
pub async fn create_tasks_ok<S, B>(app: &S, tasks: &[serde_json::Value]) -> Vec<TaskDto>
where
    S: Service<Request, Response = ServiceResponse<B>, Error = actix_web::Error>,
    B: MessageBody,
{
    let req = actix_web::test::TestRequest::post()
        .uri("/task")
        .insert_header(("requester", "test"))
        .set_json(tasks)
        .to_request();
    let resp = actix_web::test::call_service(app, req).await;
    assert_eq!(
        resp.status(),
        actix_web::http::StatusCode::CREATED,
        "POST /task should return 201 Created"
    );
    actix_web::test::read_body_json(resp).await
}

/// GET /task/{id}, assert 200, return deserialized TaskDto.
pub async fn get_task_ok<S, B>(app: &S, task_id: uuid::Uuid) -> TaskDto
where
    S: Service<Request, Response = ServiceResponse<B>, Error = actix_web::Error>,
    B: MessageBody,
{
    let req = actix_web::test::TestRequest::get()
        .uri(&format!("/task/{}", task_id))
        .to_request();
    let resp = actix_web::test::call_service(app, req).await;
    assert!(
        resp.status().is_success(),
        "GET /task/{} returned {}",
        task_id,
        resp.status()
    );
    actix_web::test::read_body_json(resp).await
}

/// Assert a task has the expected status.
pub async fn assert_task_status<S, B>(app: &S, task_id: uuid::Uuid, expected: StatusKind, msg: &str)
where
    S: Service<Request, Response = ServiceResponse<B>, Error = actix_web::Error>,
    B: MessageBody,
{
    let task = get_task_ok(app, task_id).await;
    assert_eq!(task.status, expected, "{}", msg);
}

/// Claim a task (Pending -> Running) then update it to the given terminal status.
pub async fn claim_and_complete(
    state: &AppState,
    task_id: uuid::Uuid,
    status: StatusKind,
    failure_reason: Option<String>,
) {
    let mut conn = state.pool.get().await.unwrap();
    let claimed = task_runner::db_operation::claim_task(&mut conn, &task_id)
        .await
        .unwrap();
    assert!(claimed, "Failed to claim task {}", task_id);
    let dto = task_runner::dtos::UpdateTaskDto {
        status: Some(status),
        metadata: None,
        new_success: None,
        new_failures: None,
        failure_reason,
    };
    let result = task_runner::db_operation::update_running_task(
        &state.action_executor,
        &mut conn,
        task_id,
        dto,
    )
    .await
    .unwrap();
    assert_eq!(
        result,
        task_runner::db_operation::UpdateTaskResult::Updated,
        "Failed to complete task {}",
        task_id
    );
}

/// Shortcut: claim task then mark as Success.
pub async fn succeed_task(state: &AppState, task_id: uuid::Uuid) {
    claim_and_complete(state, task_id, StatusKind::Success, None).await;
}

/// Shortcut: claim task then mark as Failure with the given reason.
pub async fn fail_task(state: &AppState, task_id: uuid::Uuid, reason: &str) {
    claim_and_complete(
        state,
        task_id,
        StatusKind::Failure,
        Some(reason.to_string()),
    )
    .await;
}

/// Helper struct for reading raw wait counters directly from DB.
#[derive(diesel::QueryableByName, Debug)]
pub struct TaskCounters {
    #[diesel(sql_type = diesel::sql_types::Integer)]
    pub wait_finished: i32,
    #[diesel(sql_type = diesel::sql_types::Integer)]
    pub wait_success: i32,
}

/// Read wait_finished and wait_success counters directly from the task table.
pub async fn read_wait_counters(pool: &task_runner::DbPool, task_id: uuid::Uuid) -> (i32, i32) {
    let mut conn = pool.get().await.unwrap();
    // Use fully qualified syntax to disambiguate from diesel::RunQueryDsl
    let c: TaskCounters = diesel_async::RunQueryDsl::get_result(
        diesel::sql_query(format!(
            "SELECT wait_finished, wait_success FROM task WHERE id = '{}'",
            task_id
        )),
        &mut *conn,
    )
    .await
    .unwrap();
    (c.wait_finished, c.wait_success)
}
