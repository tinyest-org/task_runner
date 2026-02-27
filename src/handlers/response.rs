use actix_web::HttpResponse;

use crate::validation::ValidationError;

/// Build a standardized 400 Bad Request response from validation errors.
pub fn validation_error_response(errors: &[ValidationError]) -> HttpResponse {
    let error_messages: Vec<String> = errors.iter().map(|e| e.to_string()).collect();
    HttpResponse::BadRequest().json(serde_json::json!({
        "error": "Validation failed",
        "details": error_messages
    }))
}
