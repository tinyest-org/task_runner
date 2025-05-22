//! Actix Web Diesel integration example
//!
//! Diesel v2 is not an async library, so we have to execute queries in `web::block` closures which
//! offload blocking code (like Diesel's) to a thread-pool in order to not block the server.

use std::sync::Arc;

use actix_web::{
    App, HttpResponse, HttpServer, Responder, error, get, middleware, patch, post, web,
};
use task_runner::{
    DbPool,
    action::ActionContext,
    db_operation,
    dtos::{self},
    initialize_db_pool,
};
use uuid::Uuid;

#[get("/task")]
async fn list_task(
    pool: web::Data<DbPool>,
    pagination: web::Query<dtos::PaginationDto>, // pagination
    filter: web::Query<dtos::FilterDto>,         // filter
) -> actix_web::Result<impl Responder> {
    let mut conn = pool.get().await.map_err(error::ErrorInternalServerError)?;
    let tasks = db_operation::list_task_filtered_paged(&mut conn, pagination.0, filter.0)
        .await
        // map diesel query errors to a 500 error response
        .map_err(error::ErrorInternalServerError)?;
    log::info!("got result");
    Ok(HttpResponse::Ok().json(tasks))
}

#[patch("/task/{task_id}")]
async fn update_task(
    pool: web::Data<DbPool>,
    action_context: web::Data<ActionContext>,
    task_id: web::Path<Uuid>,
    form: web::Json<dtos::UpdateTaskDto>,
) -> actix_web::Result<impl Responder> {
    // use web::block to offload blocking Diesel queries without blocking server thread
    log::debug!("Update task: {:?}", &form.status);
    let mut conn = pool.get().await.map_err(error::ErrorInternalServerError)?;
    let count = db_operation::update_task(action_context.get_ref(), &mut conn, *task_id, form.0)
        .await
        .map_err(error::ErrorInternalServerError)?;
    Ok(match count {
        1 => HttpResponse::Ok().body("Task updated successfully".to_string()),
        _ => HttpResponse::NotFound().body("Task not found".to_string()),
    })
}

/// Finds user by UID.
///
/// Extracts:
/// - the database pool handle from application data
/// - a user UID from the request path
#[get("/task/{task_id}")]
async fn get_task(
    pool: web::Data<DbPool>,
    task_id: web::Path<Uuid>,
) -> actix_web::Result<impl Responder> {
    // use web::block to offload blocking Diesel queries without blocking server thread
    let mut conn = pool.get().await.map_err(error::ErrorInternalServerError)?;
    let task = db_operation::find_detailed_task_by_id(&mut conn, *task_id)
        .await
        // map diesel query errors to a 500 error response
        .map_err(error::ErrorInternalServerError)?;

    Ok(match task {
        // user was found; return 200 response with JSON formatted user object
        Some(t) => HttpResponse::Ok().json(t),
        // user was not found; return 404 response with error message
        None => HttpResponse::NotFound().body("No task found with UID".to_string()),
    })
}

/// Creates new user.
///
/// Extracts:
/// - the database pool handle from application data
/// - a JSON form containing new user info from the request body
#[post("/task")]
async fn add_task(
    pool: web::Data<DbPool>,
    form: web::Json<dtos::NewTaskDto>,
) -> actix_web::Result<impl Responder> {
    log::info!("got query");
    // use web::block to offload blocking Diesel queries without blocking server thread
    let mut conn = pool.get().await.map_err(error::ErrorInternalServerError)?;
    log::info!("got conn");
    let user = db_operation::insert_new_task(&mut conn, form.0)
        .await
        // map diesel query errors to a 500 error response
        .map_err(error::ErrorInternalServerError)?;
    log::info!("action inserted");
    // user was added successfully; return 201 response with new user info
    Ok(HttpResponse::Created().json(user))
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    dotenvy::dotenv().ok();
    env_logger::init_from_env(env_logger::Env::new().default_filter_or("info"));
    let host_address = std::env::var("HOST_URL").expect("Env var `HOST_URL` not set");
    let action_context = Arc::from(ActionContext { host_address });

    // initialize DB pool outside of `HttpServer::new` so that it is shared across all workers
    let pool = initialize_db_pool().await;

    log::info!("starting HTTP server at http://localhost:8080");

    let p = pool.clone();

    let a = action_context.clone();
    actix_web::rt::spawn(async move {
        task_runner::workers::start_loop(a.clone().as_ref(), p).await;
    });
    let p2 = pool.clone();
    actix_web::rt::spawn(async {
        task_runner::workers::timeout_loop(p2).await;
    });

    HttpServer::new(move || {
        App::new()
            // add DB pool handle to app data; enables use of `web::Data<DbPool>` extractor
            .app_data(web::Data::new(pool.clone()))
            .app_data(web::Data::new(action_context.clone()))
            // add request logger middleware
            .wrap(middleware::Logger::default())
            .service(get_task)
            .service(add_task)
            .service(list_task)
            .service(update_task)
    })
    .bind(("127.0.0.1", 8080))?
    .run()
    .await
}
