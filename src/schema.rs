// @generated automatically by Diesel CLI.

diesel::table! {
    action (id) {
        id -> Uuid,
        kind -> Text,
        task_id -> Uuid,
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

// joinable!(users_agencies -> agencies (agencies_id));