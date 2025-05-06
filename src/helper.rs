// #[macro_export]
// macro_rules! impl_text_sql_enum {
//     ($name:ident) => {
//         use diesel::deserialize::{self, FromSql};
//         use diesel::serialize::{self, IsNull, Output, ToSql};
//         use diesel::backend::Backend;
//         // use diesel::sql_types::Text;
//         use std::io::Write;
//         use std::str::FromStr;

//         impl ToSql<Text, diesel::pg::Pg> for $name {
//             fn to_sql<B: Backend>(&self, out: &mut Output<B>) -> serialize::Result {
//                 write!(out, "{}", self.to_string())?;
//                 Ok(IsNull::No)
//             }
//         }

//         impl FromSql<Text, diesel::pg::Pg> for $name {
//             fn from_sql(bytes: diesel::pg::PgValue<'_>) -> deserialize::Result<Self> {
//                 let s = std::str::from_utf8(bytes.as_bytes())
//                     .map_err(|e| Box::<dyn std::error::Error + Send + Sync>::from(e))?;
//                 s.parse::<$name>()
//                     .map_err(|e| Box::<dyn std::error::Error + Send + Sync>::from(e))
//             }
//         }
//     };
// }