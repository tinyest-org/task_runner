pub mod action;
pub mod db_operation;
pub mod dtos;
pub mod helper;
pub mod models;
pub mod rule;
pub mod schema;
pub mod workers;

use std::time::Duration;

/// Short-hand for the database pool type to use throughout the app.
// pub type DbPool = Pool<AsyncDieselConnectionManager<>>;
use diesel::{ConnectionError, ConnectionResult};
use diesel_async::AsyncPgConnection;
use diesel_async::pooled_connection::AsyncDieselConnectionManager;
use diesel_async::pooled_connection::ManagerConfig;
use diesel_async::pooled_connection::bb8::PooledConnection;
use diesel_async::pooled_connection::bb8::{self, Pool};
use dtos::FilterDto;
use dtos::PaginationDto;
use futures_util::FutureExt;
use futures_util::future::BoxFuture;
use rustls::ClientConfig;
use rustls_platform_verifier::ConfigVerifierExt;

pub type DbPool = bb8::Pool<AsyncPgConnection>;


pub type Conn<'a> = PooledConnection<'a, AsyncPgConnection>;

/// Initialize database connection pool based on `DATABASE_URL` environment variable.
///
/// See more: <https://docs.rs/diesel/latest/diesel/r2d2/index.html>.
pub async fn initialize_db_pool() -> DbPool {
    let db_url = std::env::var("DATABASE_URL").expect("Env var `DATABASE_URL` not set");
    let mut config = ManagerConfig::default();
    config.custom_setup = Box::new(establish_connection);
    let mgr = AsyncDieselConnectionManager::<AsyncPgConnection>::new_with_config(db_url, config);

    let pool = Pool::builder()
        .max_size(10)
        .min_idle(Some(5))
        .max_lifetime(Some(Duration::from_secs(60 * 60 * 24)))
        .idle_timeout(Some(Duration::from_secs(60 * 2)))
        .build(mgr)
        .await
        .expect("failed to get pool");
    let p = pool.clone();
    log::info!("starting");
    let mut conn = p.get().await.unwrap();
    log::info!("got conn");
    db_operation::list_task_filtered_paged(&mut conn, PaginationDto::default(), FilterDto::default()).await.unwrap();
    log::info!("did query");
    pool
}

fn establish_connection(config: &str) -> BoxFuture<ConnectionResult<AsyncPgConnection>> {
    let fut = async {
        // We first set up the way we want rustls to work.
        let rustls_config = ClientConfig::with_platform_verifier();
        let tls = tokio_postgres_rustls::MakeRustlsConnect::new(rustls_config);
        let (client, conn) = tokio_postgres::connect(config, tls)
            .await
            .map_err(|e| ConnectionError::BadConnection(e.to_string()))?;

        AsyncPgConnection::try_from_client_and_connection(client, conn).await
    };
    fut.boxed()
}



