use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    dtos::NewActionDto,
    models::{Action, ActionKindEnum, Task},
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
/// When the webhook is called, the task runner appends a `?handle=<host>/task/<task_uuid>`
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

#[derive(Clone)]
pub struct ActionContext {
    pub host_address: String,
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

    #[tracing::instrument(name = "webhook_execute", skip(self, action, task), fields(task_id = %task.id, action_id = %action.id))]
    pub async fn execute(
        &self,
        action: &Action,
        task: &Task,
    ) -> Result<Option<NewActionDto>, String> {
        match action.kind {
            ActionKindEnum::Webhook => {
                let my_address = &self.ctx.host_address;
                let params: WebhookParams = serde_json::from_value(action.params.clone())
                    .map_err(|e| format!("Failed to parse webhook params: {}", e))?;
                let url = params.url;
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
                let response = request
                    .send()
                    .await
                    .map_err(|e| format!("Failed to send request: {}", e))?;
                let status = response.status();
                if status.is_redirection() {
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
                    // try to parse cancel
                    Ok(match response.text().await {
                        Ok(body) => {
                            log::info!("query with success: -> {}", &body);
                            serde_json::from_str(&body).ok()
                        }
                        Err(_) => {
                            log::info!("query with success");
                            None
                        }
                    })
                } else {
                    let body = response.text().await.unwrap_or_default();
                    log::error!("Response ({}): {}", status, body);
                    Err(format!("Request failed with status: {}", status))
                }
            }
        }
    }
}
