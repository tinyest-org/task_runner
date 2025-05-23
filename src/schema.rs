// @generated automatically by Diesel CLI.

pub mod sql_types {
    #[derive(diesel::query_builder::QueryId, Clone, diesel::sql_types::SqlType)]
    #[diesel(postgres_type(name = "action_kind"))]
    pub struct ActionKind;

    #[derive(diesel::query_builder::QueryId, Clone, diesel::sql_types::SqlType)]
    #[diesel(postgres_type(name = "status_kind"))]
    pub struct StatusKind;

    #[derive(diesel::query_builder::QueryId, Clone, diesel::sql_types::SqlType)]
    #[diesel(postgres_type(name = "trigger_kind"))]
    pub struct TriggerKind;
}

diesel::table! {
    use diesel::sql_types::*;
    use super::sql_types::ActionKind;
    use super::sql_types::TriggerKind;

    action (id) {
        id -> Uuid,
        task_id -> Uuid,
        kind -> ActionKind,
        params -> Jsonb,
        trigger -> TriggerKind,
        success -> Nullable<Bool>,
    }
}

diesel::table! {
    use diesel::sql_types::*;
    use super::sql_types::StatusKind;

    task (id) {
        // add requester field to track how want the data
        id -> Uuid,
        name -> Nullable<Text>,
        kind -> Text,
        status -> StatusKind,
        created_at -> Timestamptz,
        timeout -> Int4,
        started_at -> Nullable<Timestamptz>,
        last_updated -> Timestamptz,
        success -> Int4,
        failures -> Int4,
        metadata -> Jsonb,
        ended_at -> Nullable<Timestamptz>,
        start_condition -> Jsonb,
    }
}

diesel::joinable!(action -> task (task_id));

diesel::allow_tables_to_appear_in_same_query!(
    action,
    task,
);
