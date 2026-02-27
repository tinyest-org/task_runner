use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    dtos::NewActionDto,
    metrics,
    models::{Action, ActionKindEnum, Task, TriggerCondition, TriggerKind},
};

/// HTTP method for webhook calls.
#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub enum HttpVerb {
    /// HTTP GET
    Get,
    /// HTTP POST (most common for webhooks)
    Post,
    /// HTTP DELETE
    Delete,
    /// HTTP PUT
    Put,
    /// HTTP PATCH
    Patch,
}

impl From<HttpVerb> for reqwest::Method {
    fn from(verb: HttpVerb) -> Self {
        match verb {
            HttpVerb::Get => reqwest::Method::GET,
            HttpVerb::Post => reqwest::Method::POST,
            HttpVerb::Delete => reqwest::Method::DELETE,
            HttpVerb::Put => reqwest::Method::PUT,
            HttpVerb::Patch => reqwest::Method::PATCH,
        }
    }
}

/// Parameters for a Webhook action. This is the structure expected in `NewActionDto.params`
/// when `kind` is `Webhook`.
///
/// When the webhook is called, the ArcRun appends a `?handle=<host>/task/<task_uuid>`
/// query parameter to the URL. Your webhook handler should use this URL to report task
/// completion via `PATCH` or `PUT`.
///
/// ## Example
/// ```json
/// {
///   "url": "https://my-service.com/start-job",
///   "verb": "Post",
///   "body": {"job_type": "build", "ref": "main"},
///   "headers": {"Authorization": "Bearer secret-token"}
/// }
/// ```
#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct WebhookParams {
    /// The URL to call. Must be a valid HTTP(S) URL. Internal/private IPs are blocked (SSRF protection).
    pub url: String,
    /// HTTP method to use.
    pub verb: HttpVerb,
    /// Optional JSON body to send with the request.
    pub body: Option<serde_json::Value>,
    /// Optional HTTP headers to include. Example: `{"Authorization": "Bearer xxx", "X-Custom": "value"}`.
    pub headers: Option<HashMap<String, String>>,
}

/// Build an idempotency key for a webhook trigger event.
///
/// Format:
/// - Start → `"{task_id}:start"`
/// - End+Success → `"{task_id}:end:success"`
/// - End+Failure → `"{task_id}:end:failure"`
/// - Cancel → `"{task_id}:cancel"`
pub fn idempotency_key(
    task_id: uuid::Uuid,
    trigger: &TriggerKind,
    condition: &TriggerCondition,
) -> String {
    match trigger {
        TriggerKind::Start => format!("{}:start", task_id),
        TriggerKind::End => {
            let cond = match condition {
                TriggerCondition::Success => "success",
                TriggerCondition::Failure => "failure",
            };
            format!("{}:end:{}", task_id, cond)
        }
        TriggerKind::Cancel => format!("{}:cancel", task_id),
    }
}

#[derive(Clone)]
pub struct ActionContext {
    pub host_address: String,
    /// Controls how long a pending webhook execution can remain uncompleted
    /// before being eligible for retry.
    pub webhook_idempotency_timeout: std::time::Duration,
}

#[derive(Clone)]
pub struct ActionExecutor {
    pub ctx: ActionContext,
    pub client: reqwest::Client,
}

impl ActionExecutor {
    pub fn new(ctx: ActionContext) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("Failed to build HTTP client");
        Self { ctx, client }
    }

    #[tracing::instrument(name = "webhook_execute", level = "debug", skip(self, action, task), fields(task_id = %task.id, action_id = %action.id))]
    pub async fn execute(
        &self,
        action: &Action,
        task: &Task,
        idem_key: Option<&str>,
    ) -> Result<Option<NewActionDto>, String> {
        match action.kind {
            ActionKindEnum::Webhook => {
                let my_address = &self.ctx.host_address;
                let params: WebhookParams = serde_json::from_value(action.params.clone())
                    .map_err(|e| format!("Failed to parse webhook params: {}", e))?;
                let url = params.url;
                let trigger_str = match action.trigger {
                    TriggerKind::Start => "start",
                    TriggerKind::End => "end",
                    TriggerKind::Cancel => "cancel",
                };
                let started_at = std::time::Instant::now();
                let mut request = self.client.request(params.verb.into(), &url);
                // enable the runner to send update of the task
                request = request.query(&[("handle", format!("{}/task/{}", my_address, &task.id))]);
                if let Some(body) = params.body {
                    request = request.json(&body);
                }
                if let Some(headers) = params.headers {
                    for (key, value) in headers {
                        request = request.header(key, value);
                    }
                }
                // Inject idempotency and diagnostics headers
                if let Some(key) = idem_key {
                    request = request.header("Idempotency-Key", key);
                }
                request = request
                    .header("X-Task-Id", task.id.to_string())
                    .header("X-Task-Trigger", trigger_str);

                let response = request.send().await.map_err(|e| {
                    metrics::record_webhook_execution(
                        trigger_str,
                        "failure",
                        started_at.elapsed().as_secs_f64(),
                    );
                    format!("Failed to send request: {}", e)
                })?;
                let status = response.status();
                if status.is_redirection() {
                    metrics::record_webhook_execution(
                        trigger_str,
                        "failure",
                        started_at.elapsed().as_secs_f64(),
                    );
                    log::warn!(
                        "Webhook for task {} returned redirect status {} — redirects are disabled for SSRF protection",
                        task.id,
                        status
                    );
                    return Err(format!(
                        "Webhook returned redirect status {} — redirects are disabled",
                        status
                    ));
                }
                if status.is_success() {
                    metrics::record_webhook_execution(
                        trigger_str,
                        "success",
                        started_at.elapsed().as_secs_f64(),
                    );
                    // try to parse cancel
                    Ok(match response.text().await {
                        Ok(body) => {
                            log::info!("query with success: -> {}", &body);
                            match serde_json::from_str(&body) {
                                Ok(dto) => Some(dto),
                                Err(e) => {
                                    log::debug!(
                                        "Response body did not parse as NewActionDto: {}",
                                        e
                                    );
                                    None
                                }
                            }
                        }
                        Err(_) => {
                            log::info!("query with success");
                            None
                        }
                    })
                } else {
                    metrics::record_webhook_execution(
                        trigger_str,
                        "failure",
                        started_at.elapsed().as_secs_f64(),
                    );
                    let body = response.text().await.unwrap_or_default();
                    log::error!("Response ({}): {}", status, body);
                    Err(format!("Request failed with status: {}", status))
                }
            }
        }
    }
}
