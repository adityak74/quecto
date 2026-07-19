use serde::Deserialize;

#[derive(Debug, Deserialize, PartialEq)]
pub struct EvalConfig {
    pub id: String,
    pub suite: String,
    pub prompt_file: String,
    pub setup_script: String,
    pub graders: Vec<GraderConfig>,
    pub telemetry_thresholds: Option<TelemetryThresholds>,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum GraderConfig {
    #[serde(rename = "script")]
    Script { command: String },
    #[serde(rename = "llm_rubric")]
    LlmRubric { rubric: String },
}

#[derive(Debug, Deserialize, PartialEq)]
pub struct TelemetryThresholds {
    pub max_turns: Option<u32>,
    pub max_tokens: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_eval_yaml() {
        let yaml = r#"
id: tb_01
suite: regression
prompt_file: prompt.md
setup_script: setup.sh
graders:
  - type: script
    command: verify.sh
  - type: llm_rubric
    rubric: "Output must contain 'SUCCESS'."
telemetry_thresholds:
  max_turns: 10
"#;
        let config: EvalConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.id, "tb_01");
        assert_eq!(config.suite, "regression");
        assert_eq!(config.telemetry_thresholds.as_ref().unwrap().max_turns, Some(10));
        assert_eq!(
            config.graders,
            vec![
                GraderConfig::Script {
                    command: "verify.sh".to_string(),
                },
                GraderConfig::LlmRubric {
                    rubric: "Output must contain 'SUCCESS'.".to_string(),
                },
            ]
        );
    }
}
