use diesel::expression::AsExpression;
use diesel::{deserialize::FromSqlRow, serialize::ToSql, sql_types::Jsonb};
use serde::{Deserialize, Serialize};
use diesel::deserialize::{self, FromSql};
use diesel::pg::{Pg, PgValue};
use diesel::serialize::{self, IsNull, Output};
use std::io::Write;
use crate::models::StatusKind;

/// RuleKind is an enum that represents different kinds of rules
/// example:
/// ```
/// {
/// type: "Concurency",
/// max_concurency: 1,
/// matcher: {
///     status: "Running",
///    kind: "clustering",
///    fields: [
///        "projectId",
///    ],
///},
///},
#[derive(Debug, Clone, Serialize, PartialEq, Deserialize)]
#[serde(tag = "type")] // -> this way the "type" field will be used to determine the type of the rule
pub enum Rule {
    None, // we start immediately
    Concurency(ConcurencyRule),
}

#[derive(AsExpression, FromSqlRow, Debug, Clone, Serialize, Deserialize, PartialEq)]
#[diesel(sql_type = Jsonb)]
pub struct Rules {
    pub conditions: Vec<Rule>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Deserialize)]
pub struct ConcurencyRule {
    max_concurency: i32,
    matcher: Matcher,
}

#[derive(Debug, Clone, Serialize, PartialEq, Deserialize)]
pub struct Matcher {
    pub status: StatusKind,  // we want to match on status
    pub kind: String,        // we want to match on kind
    pub fields: Vec<String>, // we want to match on fields in the params section
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

