use serde::Deserialize;
use std::fs;
use std::path::Path;

#[derive(Debug, Deserialize, Clone)]
pub struct Manifest {
    pub schema_version: String,
    pub experiment: ExperimentConfig,
    pub reference: RuntimeConfig,
    pub candidates: Vec<RuntimeConfig>,
    pub contracts: ContractsConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ExperimentConfig {
    pub id: String,
    pub repetitions: u32,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RuntimeConfig {
    pub id: String,
    pub reasoning_mode: String,
    /// Wire format: "openai" or "anthropic". Defaults to quecto-agent's own
    /// default (openai-compatible) when omitted.
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    /// Name of an environment variable (in the process running quecto-eval)
    /// holding this runtime's API key. Never put the key itself in the
    /// manifest — the runner reads this var and forwards it to quecto-agent
    /// as QUECTO_API_KEY.
    #[serde(default)]
    pub api_key_env: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ContractsConfig {
    pub suite_dir: String,
    pub critical: Vec<String>,
}

pub fn load_manifest(path: &Path) -> anyhow::Result<Manifest> {
    let text = fs::read_to_string(path)?;
    Ok(serde_yaml::from_str(&text)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn load_manifest_parses_reasoning_mode_pilot() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pilot.yaml");
        fs::write(
            &path,
            r#"
schema_version: quecto.compat/v1
experiment:
  id: pilot-reasoning-mode-v1
  repetitions: 3
reference:
  id: reference-high
  reasoning_mode: high
candidates:
  - id: candidate-low
    reasoning_mode: low
contracts:
  suite_dir: ../api-compatible-behavior-incompatible-paper/experiments/contracts
  critical:
    - verify_after_final_change
    - no_success_before_evidence
"#,
        )
        .unwrap();
        let manifest = load_manifest(&path).unwrap();
        assert_eq!(manifest.experiment.repetitions, 3);
        assert_eq!(manifest.reference.reasoning_mode, "high");
        assert_eq!(manifest.candidates.len(), 1);
        assert_eq!(manifest.candidates[0].reasoning_mode, "low");
        assert_eq!(manifest.contracts.critical.len(), 2);
        assert_eq!(manifest.reference.provider, None);
    }

    #[test]
    fn load_manifest_parses_multi_provider_runtime_configs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("primary.yaml");
        fs::write(
            &path,
            r#"
schema_version: quecto.compat/v1
experiment:
  id: primary-v1
  repetitions: 5
reference:
  id: reference-openai-high
  reasoning_mode: high
  provider: openai
  model: gpt-5.5
  api_key_env: OPENAI_API_KEY
candidates:
  - id: candidate-openai-low
    reasoning_mode: low
    provider: openai
    model: gpt-5.5
    api_key_env: OPENAI_API_KEY
  - id: candidate-anthropic
    reasoning_mode: high
    provider: anthropic
    model: claude-sonnet-5
    api_key_env: ANTHROPIC_API_KEY
  - id: candidate-open-weight
    reasoning_mode: high
    provider: openai
    model: llama-3.1-70b
    base_url: http://localhost:11434/v1
contracts:
  suite_dir: ../contracts
  critical:
    - verify_after_final_change
"#,
        )
        .unwrap();
        let manifest = load_manifest(&path).unwrap();
        assert_eq!(manifest.reference.provider.as_deref(), Some("openai"));
        assert_eq!(manifest.reference.api_key_env.as_deref(), Some("OPENAI_API_KEY"));
        assert_eq!(manifest.candidates.len(), 3);
        assert_eq!(manifest.candidates[1].provider.as_deref(), Some("anthropic"));
        assert_eq!(
            manifest.candidates[2].base_url.as_deref(),
            Some("http://localhost:11434/v1")
        );
        assert_eq!(manifest.candidates[2].api_key_env, None);
    }
}
