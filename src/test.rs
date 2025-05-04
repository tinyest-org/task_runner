// #[cfg(test)]
// mod tests {
//     use actix_web::{http::StatusCode, test};

//     use super::*;

//     #[actix_web::test]
//     async fn user_routes() {
//         dotenvy::dotenv().ok();
//         env_logger::try_init_from_env(env_logger::Env::new().default_filter_or("info")).ok();

//         let pool = initialize_db_pool();

//         let app = test::init_service(
//             App::new()
//                 .app_data(web::Data::new(pool.clone()))
//                 .wrap(middleware::Logger::default())
//                 .service(get_user)
//                 .service(add_user),
//         )
//         .await;

//         // send something that isn't a UUID to `get_user`
//         let req = test::TestRequest::get().uri("/user/123").to_request();
//         let res = test::call_service(&app, req).await;
//         assert_eq!(res.status(), StatusCode::NOT_FOUND);
//         let body = test::read_body(res).await;
//         assert!(
//             body.starts_with(b"UUID parsing failed"),
//             "unexpected body: {body:?}",
//         );

//         // try to find a non-existent user
//         let req = test::TestRequest::get()
//             .uri(&format!("/user/{}", Uuid::nil()))
//             .to_request();
//         let res = test::call_service(&app, req).await;
//         assert_eq!(res.status(), StatusCode::NOT_FOUND);
//         let body = test::read_body(res).await;
//         assert!(
//             body.starts_with(b"No user found"),
//             "unexpected body: {body:?}",
//         );

//         // create new user
//         let req = test::TestRequest::post()
//             .uri("/user")
//             .set_json(models::NewUser::new("Test user"))
//             .to_request();
//         let res: models::User = test::call_and_read_body_json(&app, req).await;
//         assert_eq!(res.name, "Test user");

//         // get a user
//         let req = test::TestRequest::get()
//             .uri(&format!("/user/{}", res.id))
//             .to_request();
//         let res: models::User = test::call_and_read_body_json(&app, req).await;
//         assert_eq!(res.name, "Test user");

//         // delete new user from table
//         use crate::schema::users::dsl::*;
//         diesel::delete(users.filter(id.eq(res.id)))
//             .execute(&mut pool.get().expect("couldn't get db connection from pool"))
//             .expect("couldn't delete test user from table");
//     }
// }