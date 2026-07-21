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
    }
}
