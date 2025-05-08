use diesel::{r2d2, PgConnection};

pub mod  db_operation;
pub mod dtos;
pub mod helper;
pub mod models;
pub mod schema;
pub mod rule;
pub mod workers;
pub mod action;

/// Short-hand for the database pool type to use throughout the app.
pub type DbPool = r2d2::Pool<r2d2::ConnectionManager<PgConnection>>;
