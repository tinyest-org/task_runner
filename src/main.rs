//! Actix Web Diesel integration example
//!
//! Diesel v2 is not an async library, so we have to execute queries in `web::block` closures which
//! offload blocking code (like Diesel's) to a thread-pool in order to not block the server.

use actix_web::{
    App, HttpResponse, HttpServer, Responder, error, get, middleware, patch, post, web,
};
use diesel::{prelude::*, r2d2};
use task_runner::{
    db_operation,
    dtos::{self},
};
use uuid::Uuid;

/// Short-hand for the database pool type to use throughout the app.
type DbPool = r2d2::Pool<r2d2::ConnectionManager<PgConnection>>;

#[get("/task")]
async fn list_task(
    pool: web::Data<DbPool>,
    pagination: web::Query<dtos::PaginationDto>, // pagination
    filter: web::Query<dtos::FilterDto>,         // filter
) -> actix_web::Result<impl Responder> {
    // handle pagination
    // handle filter -> pending, running
    // -> return basicTaskDto
    // TODO: Implement the logic for listing tasks
    let tasks = web::block(move || {
        // note that obtaining a connection from the pool is also potentially blocking
        let mut conn = pool.get()?;

        db_operation::list_task_filtered_paged(&mut conn, pagination.0, filter.0)
    })
    .await?
    // map diesel query errors to a 500 error response
    .map_err(error::ErrorInternalServerError)?;

    Ok(HttpResponse::Ok().json(tasks))
}

// update task metadata
// update counters
// update last_updated
#[patch("/task/{task_id}")]
async fn update_task(
    pool: web::Data<DbPool>,
    task_id: web::Path<Uuid>,
    form: web::Json<dtos::UpdateTaskDto>,
) -> actix_web::Result<impl Responder> {
    // use web::block to offload blocking Diesel queries without blocking server thread
    let task = web::block(move || {
        // note that obtaining a connection from the pool is also potentially blocking
        let mut conn = pool.get()?;

        db_operation::find_detailed_task_by_id(&mut conn, *task_id)
    })
    .await?
    // map diesel query errors to a 500 error response
    .map_err(error::ErrorInternalServerError)?;
    if task.is_none() {
        return Ok(HttpResponse::NotFound().body("No task found with UID".to_string()));
    }
    // should enqueue the task update
    // should do real implem
    // -> push to redis queue
    // we need to update the last_updated field
    // if success has value -> increment success
    // if failure has value -> increment failure
    // if metadata has value -> update metadata
    Ok(HttpResponse::Ok().body("Task updated successfully".to_string()))
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
    let task = web::block(move || {
        // note that obtaining a connection from the pool is also potentially blocking
        let mut conn = pool.get()?;

        db_operation::find_detailed_task_by_id(&mut conn, *task_id)
    })
    .await?
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
    // use web::block to offload blocking Diesel queries without blocking server thread
    let user = web::block(move || {
        // note that obtaining a connection from the pool is also potentially blocking
        let mut conn = pool.get()?;

        db_operation::insert_new_task(&mut conn, form.0)
    })
    .await?
    // map diesel query errors to a 500 error response
    .map_err(error::ErrorInternalServerError)?;

    // user was added successfully; return 201 response with new user info
    Ok(HttpResponse::Created().json(user))
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    dotenvy::dotenv().ok();
    env_logger::init_from_env(env_logger::Env::new().default_filter_or("info"));

    // initialize DB pool outside of `HttpServer::new` so that it is shared across all workers
    let pool = initialize_db_pool();

    log::info!("starting HTTP server at http://localhost:8080");

    HttpServer::new(move || {
        App::new()
            // add DB pool handle to app data; enables use of `web::Data<DbPool>` extractor
            .app_data(web::Data::new(pool.clone()))
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

/// Initialize database connection pool based on `DATABASE_URL` environment variable.
///
/// See more: <https://docs.rs/diesel/latest/diesel/r2d2/index.html>.
fn initialize_db_pool() -> DbPool {
    let conn_spec = std::env::var("DATABASE_URL").expect("DATABASE_URL should be set");
    let manager = r2d2::ConnectionManager::<PgConnection>::new(conn_spec);
    r2d2::Pool::builder()
        .build(manager)
        .expect("database URL should be valid path to SQLite DB file")
}
