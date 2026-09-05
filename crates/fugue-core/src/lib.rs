use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct EngineInfo {
    pub name: &'static str,
    pub status: &'static str,
}

pub fn engine_info() -> EngineInfo {
    EngineInfo {
        name: "fugue-core",
        status: "ready",
    }
}

#[derive(Clone)]
pub struct OpenAiResponsesClient {
    http: reqwest::Client,
    api_key: String,
    endpoint: String,
    model: String,
    reasoning_effort: String,
    provider_name: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LlmProvider {
    OpenAi,
    OpenRouter,
}

#[derive(Debug, Clone)]
pub struct ResponsesRequest {
    pub instructions: String,
    pub input: String,
    /// Structured-output contract, transported as a single forced function
    /// call: providers follow tool schemas more reliably than `text.format`.
    pub json_schema: Option<ResponseJsonSchema>,
    pub tools: Vec<ResponseFunctionTool>,
    pub require_tool_call: bool,
}

#[derive(Debug, Clone)]
pub struct ResponseJsonSchema {
    pub name: String,
    pub schema: Value,
}

#[derive(Debug, Clone)]
pub struct ResponseFunctionTool {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ResponseFunctionCall {
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ResponsesOutput {
    Text(String),
    Json(Value),
    FunctionCalls(Vec<ResponseFunctionCall>),
    Invalid(String),
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LlmUsage {
    pub requested_model: String,
    pub served_model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResponsesResult {
    pub output: ResponsesOutput,
    pub usage: LlmUsage,
}

impl OpenAiResponsesClient {
    pub fn from_env(
        configured_model: Option<&str>,
        configured_reasoning_effort: Option<&str>,
        request_timeout: Duration,
    ) -> anyhow::Result<Option<Self>> {
        let reasoning_effort = configured_reasoning_effort.unwrap_or("high");
        validate_reasoning_effort(reasoning_effort)?;
        let provider_setting = std::env::var("FUGUE_LLM_PROVIDER");
        let provider = match provider_setting.as_deref() {
            Ok("openrouter") => LlmProvider::OpenRouter,
            Ok("openai") | Err(std::env::VarError::NotPresent) => LlmProvider::OpenAi,
            Ok(value) => return Err(anyhow::anyhow!("unsupported FUGUE_LLM_PROVIDER: {value}")),
            Err(error) => return Err(anyhow::anyhow!(error.to_string())),
        };
        let (key_name, endpoint, default_model, provider_name) = match provider {
            LlmProvider::OpenAi => (
                "OPENAI_API_KEY",
                "https://api.openai.com/v1/responses",
                "gpt-5.6-luna",
                "OpenAI",
            ),
            LlmProvider::OpenRouter => (
                "OPENROUTER_API_KEY",
                "https://openrouter.ai/api/v1/responses",
                "openai/gpt-5.6-luna",
                "OpenRouter",
            ),
        };
        match std::env::var(key_name) {
            Ok(api_key) if !api_key.trim().is_empty() => {
                let model = std::env::var("FUGUE_LLM_MODEL")
                    .unwrap_or_else(|_| configured_model.unwrap_or(default_model).to_string());
                if pricing_for_model(&model).is_none() {
                    return Err(anyhow::anyhow!(
                        "no configured pricing for LLM model {model}"
                    ));
                }
                Ok(Some(Self {
                    http: reqwest::Client::builder()
                        .timeout(request_timeout)
                        .build()?,
                    api_key,
                    endpoint: endpoint.to_string(),
                    model,
                    reasoning_effort: reasoning_effort.to_string(),
                    provider_name,
                }))
            }
            Ok(_) | Err(std::env::VarError::NotPresent) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn reasoning_effort(&self) -> &str {
        &self.reasoning_effort
    }

    pub async fn respond(&self, request: ResponsesRequest) -> anyhow::Result<ResponsesResult> {
        self.respond_with_profile(request, &self.model, &self.reasoning_effort)
            .await
    }

    pub async fn respond_with_profile(
        &self,
        request: ResponsesRequest,
        model: &str,
        reasoning_effort: &str,
    ) -> anyhow::Result<ResponsesResult> {
        validate_model_pricing(model)?;
        validate_reasoning_effort(reasoning_effort)?;
        let structured_call_name = request
            .json_schema
            .as_ref()
            .map(|schema| schema.name.clone());
        let expects_tool_calls = !request.tools.is_empty();
        let body = request_body(request, model, reasoning_effort);

        let response = self
            .http
            .post(&self.endpoint)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        let payload = response.text().await?;
        if !status.is_success() {
            return Err(anyhow::anyhow!(
                "{} Responses API returned {status}: {payload}",
                self.provider_name
            ));
        }

        let value: Value = serde_json::from_str(&payload)?;
        let usage = extract_usage(&value, model);
        let output = if let Some(expected) = structured_call_name {
            match extract_function_calls(&value) {
                Ok(calls) => match calls.as_slice() {
                    [call] if call.name == expected => {
                        ResponsesOutput::Json(call.arguments.clone())
                    }
                    [call] => ResponsesOutput::Invalid(format!(
                        "structured response called {} instead of the required {expected} function",
                        call.name
                    )),
                    [] => ResponsesOutput::Invalid(format!(
                        "structured response did not call the required {expected} function"
                    )),
                    calls => ResponsesOutput::Invalid(format!(
                        "structured response made {} function calls where exactly one {expected} call was required",
                        calls.len()
                    )),
                },
                Err(error) => ResponsesOutput::Invalid(error.to_string()),
            }
        } else if expects_tool_calls {
            match extract_function_calls(&value) {
                Ok(calls) if !calls.is_empty() => ResponsesOutput::FunctionCalls(calls),
                Ok(_) => ResponsesOutput::Invalid(
                    "OpenAI response did not contain a function tool call".to_string(),
                ),
                Err(error) => ResponsesOutput::Invalid(error.to_string()),
            }
        } else if let Some(output_text) = extract_output_text(&value) {
            ResponsesOutput::Text(output_text)
        } else {
            ResponsesOutput::Invalid("OpenAI response did not contain output text".to_string())
        };
        Ok(ResponsesResult { output, usage })
    }
}

fn request_body(request: ResponsesRequest, model: &str, reasoning_effort: &str) -> Value {
    let mut body = json!({
        "model": model,
        "reasoning": { "effort": reasoning_effort },
        "store": false,
        "instructions": request.instructions,
        "input": request.input,
    });

    if let Some(schema) = request.json_schema {
        body["tools"] = json!([{
            "type": "function",
            "name": schema.name,
            "description": "Deliver the required structured result as this function's arguments.",
            "parameters": schema.schema,
            "strict": true,
        }]);
        body["tool_choice"] = json!({ "type": "function", "name": schema.name });
    } else if !request.tools.is_empty() {
        body["tools"] = Value::Array(
            request
                .tools
                .into_iter()
                .map(|tool| {
                    json!({
                        "type": "function",
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": tool.parameters,
                        "strict": true,
                    })
                })
                .collect(),
        );
        if request.require_tool_call {
            body["tool_choice"] = Value::String("required".to_string());
        }
    }
    body
}

fn extract_usage(value: &Value, requested_model: &str) -> LlmUsage {
    let input_tokens = value
        .pointer("/usage/input_tokens")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let output_tokens = value
        .pointer("/usage/output_tokens")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let served_model = value
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or(requested_model)
        .to_string();
    let (input_per_million, output_per_million) =
        pricing_for_model(requested_model).expect("client validates model pricing at startup");
    LlmUsage {
        requested_model: requested_model.to_string(),
        served_model,
        input_tokens,
        output_tokens,
        cost_usd: (input_tokens as f64 * input_per_million
            + output_tokens as f64 * output_per_million)
            / 1_000_000.0,
    }
}

pub fn validate_model_pricing(model: &str) -> anyhow::Result<()> {
    if pricing_for_model(model).is_some() {
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "no configured pricing for LLM model {model}"
        ))
    }
}

fn pricing_for_model(model: &str) -> Option<(f64, f64)> {
    match model {
        "openai/gpt-5.6-luna" | "gpt-5.6-luna" => Some((1.0, 6.0)),
        "meta/muse-spark-1.1" => Some((1.25, 4.25)),
        "x-ai/grok-4.5" => Some((2.0, 6.0)),
        "z-ai/glm-5.2" => Some((0.98, 3.08)),
        "deepseek/deepseek-v4-pro" => Some((0.435, 0.87)),
        _ => None,
    }
}

pub fn validate_reasoning_effort(effort: &str) -> anyhow::Result<()> {
    match effort {
        "none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max" => Ok(()),
        _ => Err(anyhow::anyhow!(
            "unsupported LLM reasoning effort {effort}; expected one of: none, minimal, low, medium, high, xhigh, max"
        )),
    }
}

fn extract_output_text(value: &Value) -> Option<String> {
    if let Some(text) = value.get("output_text").and_then(Value::as_str) {
        return Some(text.to_string());
    }

    value
        .get("output")
        .and_then(Value::as_array)?
        .iter()
        .filter_map(|item| item.get("content").and_then(Value::as_array))
        .flat_map(|content| content.iter())
        .find_map(|content| {
            content
                .get("text")
                .and_then(Value::as_str)
                .or_else(|| content.get("output_text").and_then(Value::as_str))
                .map(ToString::to_string)
        })
}

fn extract_function_calls(value: &Value) -> anyhow::Result<Vec<ResponseFunctionCall>> {
    value
        .get("output")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("function_call"))
        .map(|item| {
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("function call missing name"))?;
            let arguments = item
                .get("arguments")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("function call missing arguments"))?;
            Ok(ResponseFunctionCall {
                name: name.to_string(),
                arguments: serde_json::from_str(arguments)?,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_uses_the_configured_uncapped_call_profile() {
        let body = request_body(
            ResponsesRequest {
                instructions: "test".to_string(),
                input: "input".to_string(),
                json_schema: None,
                tools: vec![],
                require_tool_call: false,
            },
            "gpt-5.6-luna",
            "medium",
        );

        assert_eq!(body["model"], "gpt-5.6-luna");
        assert_eq!(body["reasoning"]["effort"], "medium");
        assert_eq!(body["store"], false);
        assert!(body.get("max_output_tokens").is_none());
        assert!(body.get("max_tokens").is_none());
    }

    #[test]
    fn validates_the_responses_api_reasoning_effort_enum() {
        for effort in ["none", "minimal", "low", "medium", "high", "xhigh", "max"] {
            assert!(validate_reasoning_effort(effort).is_ok(), "{effort}");
        }
        assert!(validate_reasoning_effort("extreme").is_err());
    }

    #[test]
    fn prices_every_supported_openrouter_model_per_million_tokens() {
        assert_eq!(pricing_for_model("openai/gpt-5.6-luna"), Some((1.0, 6.0)));
        assert_eq!(pricing_for_model("meta/muse-spark-1.1"), Some((1.25, 4.25)));
        assert_eq!(pricing_for_model("x-ai/grok-4.5"), Some((2.0, 6.0)));
        assert_eq!(pricing_for_model("z-ai/glm-5.2"), Some((0.98, 3.08)));
        assert_eq!(
            pricing_for_model("deepseek/deepseek-v4-pro"),
            Some((0.435, 0.87))
        );
    }

    #[test]
    fn structured_requests_travel_as_a_single_forced_function_call() {
        let body = request_body(
            ResponsesRequest {
                instructions: "test".to_string(),
                input: "input".to_string(),
                json_schema: Some(ResponseJsonSchema {
                    name: "landscape".to_string(),
                    schema: json!({ "type": "object", "additionalProperties": false }),
                }),
                tools: vec![],
                require_tool_call: false,
            },
            "gpt-5.6-luna",
            "medium",
        );

        assert!(body.get("text").is_none());
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["name"], "landscape");
        assert_eq!(body["tools"][0]["strict"], true);
        assert_eq!(body["tool_choice"]["type"], "function");
        assert_eq!(body["tool_choice"]["name"], "landscape");
    }

    #[test]
    fn structured_extraction_requires_exactly_the_forced_call() {
        let response = json!({
            "output": [
                { "type": "reasoning" },
                { "type": "function_call", "name": "landscape", "arguments": "{\"positions\":[]}" }
            ]
        });
        let calls = extract_function_calls(&response).expect("parses");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "landscape");
        assert_eq!(calls[0].arguments, json!({ "positions": [] }));
    }
}
