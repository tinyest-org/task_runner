use crate::action::WebhookParams;
use crate::models::ActionKindEnum;
use serde_json::Value;

use super::ssrf::validate_webhook_url;

/// Validates action parameters based on action kind.
pub fn validate_action_params(kind: &ActionKindEnum, params: &Value) -> Result<(), String> {
    match kind {
        ActionKindEnum::Webhook => {
            let webhook: Result<WebhookParams, _> = serde_json::from_value(params.clone());
            match webhook {
                Ok(w) => validate_webhook_url(&w.url),
                Err(e) => Err(format!("Invalid webhook params: {}", e)),
            }
        }
    }
}
