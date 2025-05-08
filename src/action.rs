use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::models::{Action, ActionKindEnum, Task};

#[derive(Debug, Deserialize, Serialize)]
pub enum HttpVerb {
    Get, Post, Delete, Put, Patch
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

#[derive(Debug, Deserialize, Serialize)]
pub struct WebhookParamas {
    url: String,
    verb: HttpVerb,
    body: Option<serde_json::Value>,
    headers: Option<HashMap<String, String>>,
}

pub fn get_http_client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .expect("Failed to build HTTP client")
}

impl Action  {
    pub fn execute(&self, task: &Task) -> Result<bool, String> {
        match self.kind {
            ActionKindEnum::Webhook => {
                // TODO: should be properly configured
                let my_address = "http://localhost:8080";
                let params: WebhookParamas = serde_json::from_value(self.params.clone())
                    .map_err(|e| format!("Failed to parse webhook params: {}", e))?;
                let url = params.url;
                let client = get_http_client();
                let mut request = client.request(params.verb.into(), &url);
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
                let response = request.send().map_err(|e| format!("Failed to send request: {}", e))?;
                if response.status().is_success() {
                    Ok(true)
                } else {
                    Err(format!("Request failed with status: {}", response.status()))
                }
            }
        }
    }
}
