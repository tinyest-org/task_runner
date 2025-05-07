use diesel::expression::AsExpression;
use diesel::{deserialize::FromSqlRow, serialize::ToSql, sql_types::Jsonb};
use serde::{Deserialize, Serialize};

use crate::models::StatusKind;

/// RuleKind is an enum that represents different kinds of rules
#[derive(Debug, Clone, Serialize, PartialEq, Deserialize)]
pub enum Rule {
    None, // we start immediately
    Concurency(ConcurencyRule),
}

#[derive(AsExpression, FromSqlRow, Debug, Clone, Serialize, Deserialize, PartialEq)]
#[diesel(sql_type = Jsonb)]
pub struct Rules {
    pub rules: Vec<Rule>,
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

use diesel::deserialize::{self, FromSql};
use diesel::pg::{Pg, PgValue};
use diesel::serialize::{self, IsNull, Output};
use std::io::Write;

impl ToSql<Jsonb, Pg> for Rules {
    fn to_sql<'b>(&'b self, out: &mut Output<'b, '_, Pg>) -> serialize::Result {
        // <serde_json::Value as FromSql<Jsonb, Pg>>::to_sql(
        // we should be able to use jsonb implementation but the borrow checker
        // doesn't like it
        out.write(&[1])?; // JSONB version
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

// Instead of generic implementations, let's create a wrapper type to handle JSON serialization

// /// A wrapper type that can be used to store any Serialize/Deserialize type as JSONB
// #[derive(Debug, Clone, PartialEq)]
// pub struct JsonData<T>(pub T) where T: Serialize + for<'de> Deserialize<'de>;

// impl<T> ToSql<Jsonb, Pg> for JsonData<T> 
// where 
//     T: Serialize,
// {
//     fn to_sql<'b>(&'b self, out: &mut Output<'b, '_, Pg>) -> serialize::Result {
//         let json = serde_json::to_value(&self.0)?;
//         <serde_json::Value as ToSql<Jsonb, Pg>>::to_sql(&json, out)
//     }
// }

// impl<T> FromSql<Jsonb, Pg> for JsonData<T> 
// where 
//     T: for<'de> Deserialize<'de>,
// {
//     fn from_sql(bytes: PgValue<'_>) -> deserialize::Result<Self> {
//         let value = <serde_json::Value as FromSql<Jsonb, Pg>>::from_sql(bytes)?;
//         let data = serde_json::from_value(value)?;
//         Ok(JsonData(data))
//     }
// }

// // Convenience methods
// impl<T> JsonData<T>
// where
//     T: Serialize + for<'de> Deserialize<'de>,
// {
//     pub fn new(data: T) -> Self {
//         JsonData(data)
//     }
    
//     pub fn into_inner(self) -> T {
//         self.0
//     }
// }