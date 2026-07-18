use crate::BoxErr;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

/// Which wire format `HttpModel` speaks to the configured endpoint.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    #[default]
    #[serde(rename = "openai")]
    OpenAiCompatible,
    Anthropic,
}

impl Provider {
    /// The path segment to append to `base_url` for this provider's completion endpoint.
    pub fn path_suffix(&self) -> &'static str {
        match self {
            Provider::OpenAiCompatible => "chat/completions",
            Provider::Anthropic => "messages",
        }
    }
}

impl FromStr for Provider {
    type Err = BoxErr;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "openai" | "openai-compatible" | "openai_compatible" => Ok(Self::OpenAiCompatible),
            "anthropic" | "claude" => Ok(Self::Anthropic),
            other => Err(format!("unknown provider: {other}").into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_provider_is_openai_compatible() {
        assert_eq!(Provider::default(), Provider::OpenAiCompatible);
    }

    #[test]
    fn path_suffix_differs_per_provider() {
        assert_eq!(Provider::OpenAiCompatible.path_suffix(), "chat/completions");
        assert_eq!(Provider::Anthropic.path_suffix(), "messages");
    }

    #[test]
    fn parses_known_aliases_case_insensitively() {
        for alias in ["openai", "OpenAI", "openai-compatible", "openai_compatible"] {
            assert_eq!(alias.parse::<Provider>().unwrap(), Provider::OpenAiCompatible);
        }
        for alias in ["anthropic", "Anthropic", "claude", "CLAUDE"] {
            assert_eq!(alias.parse::<Provider>().unwrap(), Provider::Anthropic);
        }
    }

    #[test]
    fn rejects_unknown_providers() {
        assert!("bedrock".parse::<Provider>().is_err());
    }
}
