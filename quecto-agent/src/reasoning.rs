use crate::BoxErr;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::str::FromStr;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningMode {
    None,
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
}

impl ReasoningMode {
    pub fn effort_str(&self) -> &'static str {
        match self {
            ReasoningMode::None => "none",
            ReasoningMode::Minimal => "minimal",
            ReasoningMode::Low => "low",
            ReasoningMode::Medium => "medium",
            ReasoningMode::High => "high",
            ReasoningMode::XHigh => "xhigh",
        }
    }
}

impl FromStr for ReasoningMode {
    type Err = BoxErr;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "none" => Ok(Self::None),
            "minimal" => Ok(Self::Minimal),
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            "xhigh" => Ok(Self::XHigh),
            other => Err(format!("unknown reasoning mode: {other}").into()),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CompletionOptions {
    pub reasoning_mode: Option<ReasoningMode>,
}

pub fn parse_env_reasoning_mode() -> Result<Option<ReasoningMode>, BoxErr> {
    match std::env::var("QUECTO_REASONING_MODE") {
        Ok(value) if !value.trim().is_empty() => Ok(Some(value.parse()?)),
        Ok(_) => Ok(None),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(e) => Err(Box::new(e)),
    }
}

pub fn reasoning_payload(mode: ReasoningMode) -> Value {
    json!({"reasoning_effort": mode.effort_str()})
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CompletionTelemetry {
    pub requested_reasoning_mode: Option<ReasoningMode>,
    pub provider_reasoning_parameters: Option<Value>,
    pub reasoning_parameters_sent: bool,
    pub reasoning_content_available: bool,
    pub actual_reasoning_tokens: Option<u64>,
}

pub fn apply_reasoning_mode(
    body: &mut Value,
    endpoint_url: &str,
    mode: Option<ReasoningMode>,
) -> Option<Value> {
    let mode = mode?;
    let endpoint = endpoint_url
        .split(['?', '#'])
        .next()
        .unwrap_or(endpoint_url)
        .trim_end_matches('/');
    if !endpoint.ends_with("/chat/completions") {
        return None;
    }

    let payload = reasoning_payload(mode);
    let obj = body.as_object_mut()?;
    obj.insert(
        "reasoning_effort".into(),
        payload["reasoning_effort"].clone(),
    );
    Some(payload)
}

/// Anthropic's `thinking.budget_tokens` for each `ReasoningMode`.
/// `None` omits the `thinking` parameter entirely (thinking disabled).
/// Anthropic's minimum `budget_tokens` is 1024, hence `Minimal` maps there
/// rather than to 0.
pub fn anthropic_thinking_budget(mode: ReasoningMode) -> Option<u64> {
    match mode {
        ReasoningMode::None => None,
        ReasoningMode::Minimal => Some(1024),
        ReasoningMode::Low => Some(4000),
        ReasoningMode::Medium => Some(10000),
        ReasoningMode::High => Some(24000),
        ReasoningMode::XHigh => Some(32000),
    }
}

/// Inject Anthropic's `thinking: {"type":"enabled","budget_tokens":N}` into
/// the request body for the given mode, if any. Returns the injected
/// payload (for telemetry), or `None` if no mode was requested or the mode
/// maps to no budget (`ReasoningMode::None`).
pub fn apply_anthropic_thinking(body: &mut Value, mode: Option<ReasoningMode>) -> Option<Value> {
    let budget = anthropic_thinking_budget(mode?)?;
    let payload = json!({"type": "enabled", "budget_tokens": budget});
    body.as_object_mut()?.insert("thinking".to_string(), payload.clone());
    Some(json!({"thinking": payload}))
}

pub fn parse_reasoning_tokens(resp: &Value) -> Option<u64> {
    let usage = resp.get("usage")?;
    [
        usage
            .get("completion_tokens_details")
            .and_then(|details| details.get("reasoning_tokens")),
        usage
            .get("output_tokens_details")
            .and_then(|details| details.get("reasoning_tokens")),
        usage.get("reasoning_tokens"),
    ]
    .into_iter()
    .flatten()
    .find_map(Value::as_u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_reasoning_modes_case_insensitively() {
        assert_eq!(
            "none".parse::<ReasoningMode>().unwrap(),
            ReasoningMode::None
        );
        assert_eq!(
            "Minimal".parse::<ReasoningMode>().unwrap(),
            ReasoningMode::Minimal
        );
        assert_eq!("LOW".parse::<ReasoningMode>().unwrap(), ReasoningMode::Low);
        assert_eq!(
            "medium".parse::<ReasoningMode>().unwrap(),
            ReasoningMode::Medium
        );
        assert_eq!(
            "high".parse::<ReasoningMode>().unwrap(),
            ReasoningMode::High
        );
        assert_eq!(
            "xhigh".parse::<ReasoningMode>().unwrap(),
            ReasoningMode::XHigh
        );
    }

    #[test]
    fn rejects_unknown_reasoning_modes() {
        assert!("turbo".parse::<ReasoningMode>().is_err());
    }

    #[test]
    fn anthropic_budget_ladder_covers_every_mode() {
        assert_eq!(anthropic_thinking_budget(ReasoningMode::None), None);
        assert_eq!(anthropic_thinking_budget(ReasoningMode::Minimal), Some(1024));
        assert_eq!(anthropic_thinking_budget(ReasoningMode::Low), Some(4000));
        assert_eq!(anthropic_thinking_budget(ReasoningMode::Medium), Some(10000));
        assert_eq!(anthropic_thinking_budget(ReasoningMode::High), Some(24000));
        assert_eq!(anthropic_thinking_budget(ReasoningMode::XHigh), Some(32000));
    }

    #[test]
    fn applies_thinking_payload_to_body() {
        let mut body = json!({"model": "claude-x", "messages": []});

        let payload = apply_anthropic_thinking(&mut body, Some(ReasoningMode::High)).unwrap();

        assert_eq!(
            body["thinking"],
            json!({"type": "enabled", "budget_tokens": 24000})
        );
        assert_eq!(payload, json!({"thinking": {"type": "enabled", "budget_tokens": 24000}}));
    }

    #[test]
    fn none_mode_omits_thinking_entirely() {
        let mut body = json!({"model": "claude-x", "messages": []});

        let payload = apply_anthropic_thinking(&mut body, Some(ReasoningMode::None));

        assert!(payload.is_none());
        assert!(body.get("thinking").is_none());
    }

    #[test]
    fn no_mode_omits_thinking_entirely() {
        let mut body = json!({"model": "claude-x", "messages": []});

        let payload = apply_anthropic_thinking(&mut body, None);

        assert!(payload.is_none());
        assert!(body.get("thinking").is_none());
    }
}
