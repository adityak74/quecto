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
    json!({"reasoning": {"effort": mode.effort_str()}})
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CompletionTelemetry {
    pub requested_reasoning_mode: Option<ReasoningMode>,
    pub provider_reasoning_parameters: Option<Value>,
    pub reasoning_mode_applied: bool,
    pub actual_reasoning_tokens: Option<u64>,
}

pub fn apply_reasoning_mode(body: &mut Value, mode: Option<ReasoningMode>) -> Option<Value> {
    let mode = mode?;
    let payload = reasoning_payload(mode);
    if let Some(obj) = body.as_object_mut() {
        obj.insert("reasoning".into(), payload["reasoning"].clone());
    }
    Some(payload)
}

pub fn parse_reasoning_tokens(resp: &Value) -> Option<u64> {
    resp.get("usage")
        .and_then(|u| u.get("completion_tokens_details"))
        .and_then(|d| d.get("reasoning_tokens"))
        .and_then(Value::as_u64)
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
}
