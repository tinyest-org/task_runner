pub mod action;
pub mod db_operation;
pub mod dtos;
pub mod helper;
pub mod models;
pub mod rule;
pub mod schema;
pub mod workers;

use diesel_async::AsyncPgConnection;
/// Short-hand for the database pool type to use throughout the app.
// pub type DbPool = Pool<AsyncDieselConnectionManager<>>;
use diesel_async::pooled_connection::{AsyncDieselConnectionManager};

pub type DbPool = ::deadpool::managed::Pool<AsyncDieselConnectionManager<AsyncPgConnection>>;

pub type Conn = deadpool::managed::Object<
    diesel_async::pooled_connection::AsyncDieselConnectionManager<diesel_async::AsyncPgConnection>,
>;
