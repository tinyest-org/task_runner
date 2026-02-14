use crate::models::StatusKind;
use diesel::deserialize::{self, FromSql};
use diesel::expression::AsExpression;
use diesel::pg::{Pg, PgValue};
use diesel::serialize::{self, IsNull, Output};
use diesel::{deserialize::FromSqlRow, serialize::ToSql, sql_types::Jsonb};
use serde::{Deserialize, Serialize};
use std::io::Write;
use utoipa::ToSchema;

/// A concurrency control strategy. Discriminated by the `type` JSON field.
///
/// Currently only `Concurency` is supported. When the worker picks up a Pending task, it checks
/// its rules against all currently Running tasks. If the concurrency limit is reached, the task
/// stays Pending until a slot opens.
///
/// ## Example
/// ```json
/// {
///   "type": "Concurency",
///   "max_concurency": 1,
///   "matcher": {
///     "status": "Running",
///     "kind": "clustering",
///     "fields": ["projectId"]
///   }
/// }
/// ```
/// This means: at most 1 task of kind "clustering" with the same `projectId` metadata value
/// can be Running at the same time.
#[derive(Debug, Clone, Serialize, PartialEq, Deserialize, Hash, Eq, ToSchema)]
#[serde(tag = "type")]
pub enum Strategy {
    /// Limits the number of concurrent Running tasks matching specific criteria.
    Concurency(ConcurencyRule),
}

/// Array of concurrency strategies attached to a task. Empty array means no concurrency constraints.
#[derive(AsExpression, FromSqlRow, Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
#[diesel(sql_type = Jsonb)]
#[derive(Default)]
pub struct Rules(pub Vec<Strategy>);

/// Defines a concurrency limit: "at most `max_concurency` tasks matching `matcher` can run simultaneously."
#[derive(Debug, Clone, Serialize, PartialEq, Deserialize, Hash, Eq, ToSchema)]
pub struct ConcurencyRule {
    /// Maximum number of concurrent tasks allowed. Must be a positive integer.
    /// Example: 1 means "only one at a time" (mutual exclusion).
    pub max_concurency: i32,
    /// Criteria for which Running tasks count toward the limit.
    pub matcher: Matcher,
}

/// Criteria for matching tasks. Used in concurrency rules and deduplication.
///
/// Two tasks "match" if they have the same `status`, same `kind`, and identical values for all
/// keys listed in `fields` (looked up in the task's `metadata` JSON).
#[derive(Debug, Clone, Serialize, PartialEq, Deserialize, Hash, Eq, ToSchema)]
pub struct Matcher {
    /// The task status to match on. For concurrency rules, this is typically `Running`.
    pub status: StatusKind,
    /// The task kind to match on. Must be non-empty.
    pub kind: String,
    /// Metadata field keys to compare. Two tasks match only if their metadata values are equal
    /// for ALL listed keys. Example: `["projectId", "environment"]` means two tasks match only
    /// if both their `projectId` and `environment` metadata values are identical.
    pub fields: Vec<String>,
}

impl ToSql<Jsonb, Pg> for Rules {
    fn to_sql<'b>(&'b self, out: &mut Output<'b, '_, Pg>) -> serialize::Result {
        // <serde_json::Value as FromSql<Jsonb, Pg>>::to_sql(
        // we should be able to use jsonb implementation but the borrow checker
        // doesn't like it
        out.write_all(&[1])?; // JSONB version
        out.write_all(&serde_json::to_vec(&self)?)?;
        Ok(IsNull::No)
    }
}

impl FromSql<Jsonb, Pg> for Rules {
    fn from_sql(bytes: PgValue<'_>) -> deserialize::Result<Self> {
        let value = <serde_json::Value as FromSql<Jsonb, Pg>>::from_sql(bytes)?;
        Ok(serde_json::from_value(value)?)
    }
}
