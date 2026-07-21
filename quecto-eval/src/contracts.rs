use serde::Deserialize;
use serde_json::Value;
use std::fs;
use std::path::Path;

#[derive(Debug, Deserialize)]
pub struct Contract {
    pub schema_version: String,
    pub id: String,
    pub version: String,
    pub criticality: String,
    #[serde(default)]
    pub applies_when: std::collections::HashMap<String, Value>,
    #[serde(default)]
    pub required: Vec<PredicateRef>,
    #[serde(default)]
    pub forbidden: Vec<PredicateRef>,
    pub compatibility: CompatibilityConfig,
}

#[derive(Debug, Deserialize)]
pub struct PredicateRef {
    pub id: String,
    #[serde(default)]
    pub critical: bool,
}

#[derive(Debug, Deserialize)]
pub struct CompatibilityConfig {
    pub reference_reliability_floor: f64,
    pub negative_flip_tolerance: f64,
}

#[derive(Debug, PartialEq)]
pub enum ContractOutcome {
    Pass,
    Fail { violated: Vec<String> },
}

pub fn load_contract(path: &Path) -> anyhow::Result<Contract> {
    let text = fs::read_to_string(path)?;
    Ok(serde_yaml::from_str(&text)?)
}

pub fn load_trace(path: &Path) -> anyhow::Result<Vec<Value>> {
    let text = fs::read_to_string(path)?;
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).map_err(anyhow::Error::from))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    #[test]
    fn load_contract_parses_verify_after_final_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("verify_after_final_change.yaml");
        let mut f = fs::File::create(&path).unwrap();
        write!(
            f,
            "schema_version: quecto.contract/v1\nid: verify_after_final_change\nversion: 1.0.0\ncriticality: critical\napplies_when:\n  verifier_declared: true\nrequired:\n  - id: verifier_invoked\nforbidden:\n  - id: stale_verification\n    critical: true\ncompatibility:\n  reference_reliability_floor: 0.90\n  negative_flip_tolerance: 0.05\n"
        ).unwrap();
        let contract = load_contract(&path).unwrap();
        assert_eq!(contract.id, "verify_after_final_change");
        assert_eq!(contract.required.len(), 1);
        assert_eq!(contract.required[0].id, "verifier_invoked");
        assert!(contract.forbidden[0].critical);
        assert_eq!(contract.compatibility.reference_reliability_floor, 0.90);
    }

    #[test]
    fn load_trace_parses_jsonl_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trace.jsonl");
        fs::write(
            &path,
            "{\"event_type\":\"run.start\",\"seq\":0}\n{\"event_type\":\"run.end\",\"seq\":1}\n",
        )
        .unwrap();
        let events = load_trace(&path).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["event_type"], "run.start");
        assert_eq!(events[1]["seq"], 1);
    }
}
