use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct EvalConfig {
    pub id: String,
    pub suite: String,
    pub prompt_file: String,
    pub setup_script: String,
    pub graders: Vec<GraderConfig>,
    pub telemetry_thresholds: Option<TelemetryThresholds>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum GraderConfig {
    #[serde(rename = "script")]
    Script { command: String },
    #[serde(rename = "llm_rubric")]
    LlmRubric { rubric: String },
}

#[derive(Debug, Deserialize)]
pub struct TelemetryThresholds {
    pub max_turns: Option<u32>,
    pub max_tokens: Option<u32>,
}
