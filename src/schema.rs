// @generated automatically by Diesel CLI.

diesel::table! {
    task (id) {
        id -> Uuid,
        name -> Text,
        kind -> Text,
        status -> Text,
        timeout -> Int4,
    }
}
