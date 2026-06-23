//! Minimal Claude Messages API client (raw reqwest). No SDK — just the tool-use
//! request/response shape we need for the Traverser loop.

use serde_json::{json, Value};

pub struct Client {
    http: reqwest::Client,
}

impl Client {
    pub fn new() -> Self {
        Client { http: reqwest::Client::new() }
    }

    /// One `POST /v1/messages` round-trip. Returns the parsed response JSON (with
    /// `content`, `stop_reason`, …). Errors carry the API's error body.
    pub async fn create_message(
        &self,
        api_key: &str,
        model: &str,
        system: &str,
        tools: &[Value],
        messages: &[Value],
    ) -> anyhow::Result<Value> {
        let body = json!({
            "model": model,
            "max_tokens": 4096,
            "system": system,
            "tools": tools,
            "messages": messages,
        });
        let resp = self
            .http
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;
        let status = resp.status();
        let v: Value = resp.json().await?;
        if !status.is_success() {
            anyhow::bail!("anthropic API error {}: {}", status, v);
        }
        Ok(v)
    }
}
