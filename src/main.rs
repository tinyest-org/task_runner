//! Actix Web Diesel integration example
//!
//! Diesel v2 is not an async library, so we have to execute queries in `web::block` closures which
//! offload blocking code (like Diesel's) to a thread-pool in order to not block the server.

use std::sync::Arc;

use actix_web::{
    App, HttpResponse, HttpServer, Responder, error, get, middleware, patch, post, web,
};
use actix_web_prometheus::PrometheusMetricsBuilder;
use task_runner::{
    DbPool,
    action::{ActionContext, ActionExecutor},
    db_operation,
    dtos::{self},
    helper::Requester,
    initialize_db_pool,
};
use uuid::Uuid;

#[get("/task")]
async fn list_task(
    state: web::Data<AppState>,
    pagination: web::Query<dtos::PaginationDto>, // pagination
    filter: web::Query<dtos::FilterDto>,         // filter
) -> actix_web::Result<impl Responder> {
    let mut conn = state
        .pool
        .get()
        .await
        .map_err(error::ErrorInternalServerError)?;
    let tasks = db_operation::list_task_filtered_paged(&mut conn, pagination.0, filter.0)
        .await
        // map diesel query errors to a 500 error response
        .map_err(error::ErrorInternalServerError)?;
    Ok(HttpResponse::Ok().json(tasks))
}

#[patch("/task/{task_id}")]
async fn update_task(
    state: web::Data<AppState>,
    // evaluator: web::Data<ActionExecutor>,
    task_id: web::Path<Uuid>,
    form: web::Json<dtos::UpdateTaskDto>,
) -> actix_web::Result<impl Responder> {
    log::info!("Update task: {:?}", &form.status);
    let mut conn = state
        .pool
        .get()
        .await
        .map_err(error::ErrorInternalServerError)?;
    let count = db_operation::update_task(&state.action_executor, &mut conn, *task_id, form.0)
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
    state: web::Data<AppState>,
    task_id: web::Path<Uuid>,
) -> actix_web::Result<impl Responder> {
    // use web::block to offload blocking Diesel queries without blocking server thread
    let mut conn = state
        .pool
        .get()
        .await
        .map_err(error::ErrorInternalServerError)?;
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
    state: web::Data<AppState>,
    form: web::Json<Vec<dtos::NewTaskDto>>,
    requester: web::Header<Requester>,
) -> actix_web::Result<impl Responder> {
    log::debug!("got query from {}", requester.0);
    let mut conn = state
        .pool
        .get()
        .await
        .map_err(error::ErrorInternalServerError)?;
    // TODO: check the dedupe strategy
    let mut result = vec![];
    for i in form.0.into_iter() {
        let task = db_operation::insert_new_task(&mut conn, i)
            .await
            // map diesel query errors to a 500 error response
            .map_err(error::ErrorInternalServerError)?;
        if let Some(t) = task {
            result.push(t);
        }
    }
    if result.len() > 0 {
        Ok(HttpResponse::Created().json(result))
    } else {
        Ok(HttpResponse::NoContent().finish())
    }
    // user was added successfully; return 201 response with new user info
}

#[derive(Clone)]
struct AppState {
    pub action_executor: ActionExecutor,
    pub pool: DbPool,
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    dotenvy::dotenv().ok();
    env_logger::init_from_env(env_logger::Env::new().default_filter_or("info"));
    let host_address = std::env::var("HOST_URL").expect("Env var `HOST_URL` not set");
    let port = 8085;

    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");
    log::info!("starting HTTP server at http://0.0.0.0:{port}");
    log::info!("Using public url {}", &host_address);
    // CryptoProvider::install_default();
    // in order to let applications know how to respond back
    let pool = initialize_db_pool().await;
    let app_data = AppState {
        pool: pool.clone(),
        action_executor: ActionExecutor {
            ctx: ActionContext {
                host_address: host_address.to_owned(),
            },
        },
    };
    let action_context = Arc::from(ActionExecutor {
        ctx: ActionContext { host_address },
    });

    // initialize DB pool outside of `HttpServer::new` so that it is shared across all workers

    let p = pool.clone();

    let a = action_context.clone();
    actix_web::rt::spawn(async move {
        task_runner::workers::start_loop(a.clone().as_ref(), p).await;
    });

    let p2 = pool.clone();
    actix_web::rt::spawn(async {
        task_runner::workers::timeout_loop(p2).await;
    });

    let prometheus = PrometheusMetricsBuilder::new("api")
        .endpoint("/metrics")
        // .const_labels(labels)
        .build()
        .unwrap();

    HttpServer::new(move || {
        App::new()
            // add DB pool handle to app data; enables use of `web::Data<DbPool>` extractor
            .app_data(web::Data::new(app_data.clone()))
            .wrap(prometheus.clone())
            .wrap(middleware::Logger::default())
            .service(get_task)
            .service(add_task)
            .service(list_task)
            .service(update_task)
    })
    .bind(("0.0.0.0", port))?
    .run()
    .await
}
