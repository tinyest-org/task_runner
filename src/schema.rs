// @generated automatically by Diesel CLI.

pub mod sql_types {
    #[derive(diesel::query_builder::QueryId, Clone, diesel::sql_types::SqlType)]
    #[diesel(postgres_type(name = "action_kind"))]
    pub struct ActionKind;
}

diesel::table! {
    use diesel::sql_types::*;
    use super::sql_types::ActionKind;

    action (id) {
        id -> Uuid,
        task_id -> Uuid,
        kind -> ActionKind,
    }
}

diesel::table! {
    task (id) {
        id -> Uuid,
        name -> Text,
        kind -> Text,
        status -> Text,
        timeout -> Int4,
    }
}

diesel::allow_tables_to_appear_in_same_query!(
    action,
    task,
);
