use crate::common::*;

#[tokio::test]
async fn test_health_check() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    let req = actix_web::test::TestRequest::get()
        .uri("/health")
        .to_request();
    let resp = actix_web::test::call_service(&app, req).await;

    assert!(resp.status().is_success());

    let body: serde_json::Value = actix_web::test::read_body_json(resp).await;
    assert_eq!(body["status"], "ok");
}

#[tokio::test]
async fn test_readiness_check() {
    let (_g, state) = setup_test_app().await;
    let app = test_service!(state);

    let req = actix_web::test::TestRequest::get()
        .uri("/ready")
        .to_request();
    let resp = actix_web::test::call_service(&app, req).await;

    assert!(resp.status().is_success());

    let body: serde_json::Value = actix_web::test::read_body_json(resp).await;
    assert_eq!(body["status"], "ready");
}
