//! Actix Web Diesel integration example
//!
//! Diesel v2 is not an async library, so we have to execute queries in `web::block` closures which
//! offload blocking code (like Diesel's) to a thread-pool in order to not block the server.

use std::sync::Arc;

use actix_web::{
    App, HttpResponse, HttpServer, Responder, error, get,
    http::header::{Header, TryIntoHeaderValue},
    middleware, patch, post, web,
};
use actix_web_prometheus::PrometheusMetricsBuilder;
use rustls::crypto::CryptoProvider;
use task_runner::{
    DbPool,
    action::{ActionContext, ActionExecutor},
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
    Ok(HttpResponse::Ok().json(tasks))
}

#[patch("/task/{task_id}")]
async fn update_task(
    pool: web::Data<DbPool>,
    evaluator: web::Data<ActionExecutor>,
    task_id: web::Path<Uuid>,
    form: web::Json<dtos::UpdateTaskDto>,
) -> actix_web::Result<impl Responder> {
    // use web::block to offload blocking Diesel queries without blocking server thread
    log::debug!("Update task: {:?}", &form.status);
    let mut conn = pool.get().await.map_err(error::ErrorInternalServerError)?;
    let count = db_operation::update_task(evaluator.get_ref(), &mut conn, *task_id, form.0)
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

use actix_web::http::header::{HeaderName, HeaderValue};

use std::fmt;
#[derive(Debug)]
struct Requester(String);

impl Header for Requester {
    fn name() -> HeaderName {
        HeaderName::from_static("requester")
    }

    fn parse<M>(msg: &M) -> Result<Self, error::ParseError>
    where
        M: actix_web::HttpMessage,
    {
        if let Some(header_value) = msg.headers().get(HeaderName::from_static("requester")) {
            header_value
                .to_str()
                .map(|s| Requester(s.to_owned()))
                .map_err(|_| error::ParseError::Header)
        } else {
            Err(error::ParseError::Header)
        }
    }
}

impl fmt::Display for Requester {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl TryIntoHeaderValue for Requester {
    type Error = actix_web::http::header::InvalidHeaderValue;

    fn try_into_value(self) -> Result<HeaderValue, Self::Error> {
        HeaderValue::from_str(&self.0)
    }
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
    requester: web::Header<Requester>,
) -> actix_web::Result<impl Responder> {
    log::debug!("got query from {}", requester.0);
    // use web::block to offload blocking Diesel queries without blocking server thread
    let mut conn = pool.get().await.map_err(error::ErrorInternalServerError)?;
    let user = db_operation::insert_new_task(&mut conn, form.0)
        .await
        // map diesel query errors to a 500 error response
        .map_err(error::ErrorInternalServerError)?;
    // user was added successfully; return 201 response with new user info
    Ok(HttpResponse::Created().json(user))
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    dotenvy::dotenv().ok();
    env_logger::init_from_env(env_logger::Env::new().default_filter_or("info"));
    let host_address = std::env::var("HOST_URL").expect("Env var `HOST_URL` not set");
    rustls::crypto::ring::default_provider().install_default().expect("Failed to install rustls crypto provider");
    // CryptoProvider::install_default();
    // in order to let applications know how to respond back
    let action_context = Arc::from(ActionExecutor {
        ctx: ActionContext { host_address },
    });

    // initialize DB pool outside of `HttpServer::new` so that it is shared across all workers
    let pool = initialize_db_pool().await;
    let port = 8080;

    log::info!("starting HTTP server at http://localhost:{port}");

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
            .app_data(web::Data::new(pool.clone()))
            .app_data(web::Data::new(action_context.clone()))
            .wrap(prometheus.clone())
            // add request logger middleware
            .wrap(middleware::Logger::default())
            // should add health metrics
            .service(get_task)
            .service(add_task)
            .service(list_task)
            .service(update_task)
    })
    .bind(("0.0.0.0", port))?
    .run()
    .await
}
