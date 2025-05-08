use std::collections::HashMap;

use actix_web::{
    App, HttpResponse, HttpServer, Responder, error, get, middleware, patch, post, rt, web,
};
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
struct Handle {
    handle: String,
}

#[derive(Deserialize, Debug)]
struct TestData {
    wait_for: i32,
}

#[post("/task")]
async fn do_task(
    // need to received  a task handle as query param named "handle"
    body: web::Json<TestData>,  // added parameter to receive query
    handle: web::Query<Handle>, // added parameter to receive query
) -> actix_web::Result<impl Responder> {
    log::info!("Handle is: {:?}", handle.handle);
    log::info!("Received body: {:?}", body);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .expect("Failed to build HTTP client");
    // respond 10 seconds later using the handle
    rt::spawn(async move {
        rt::time::sleep(std::time::Duration::from_secs(body.wait_for as u64)).await; // added delay
        let mut i = 0;
        while i < 10 {
            let mut request = client.patch(&handle.handle);
            let body = json!({
                // "status": "Success",
                "new_success": 1,
            });
            request = request.json(&body);
            let result = request.send().await.map_err(|e| {
                log::error!("Failed to send request: {}", e);
                error::ErrorInternalServerError("Failed to send request")
            });
            if let Err(err) = result {
                log::error!("Error occurred: {:?}", err);
            }
            rt::time::sleep(std::time::Duration::from_secs(1)).await; // added delay
            i += 1;
        }
        let mut request = client.patch(&handle.handle);
        let body = json!({
            "status": "Success",
            "new_success": 1,
        });
        request = request.json(&body);
        let result = request.send().await.map_err(|e| {
            log::error!("Failed to send request: {}", e);
            error::ErrorInternalServerError("Failed to send request")
        });
    });
    Ok(HttpResponse::Ok())
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    dotenvy::dotenv().ok();
    env_logger::init_from_env(env_logger::Env::new().default_filter_or("info"));

    HttpServer::new(move || {
        App::new()
            .wrap(middleware::Logger::default())
            .service(do_task)
    })
    .bind(("127.0.0.1", 9090))?
    .run()
    .await
}
