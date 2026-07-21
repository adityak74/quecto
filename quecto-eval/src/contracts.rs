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

fn seq_of(e: &Value) -> u64 {
    e.get("seq").and_then(|v| v.as_u64()).unwrap_or(0)
}

fn events_of_type<'a>(events: &'a [Value], event_type: &str) -> Vec<&'a Value> {
    events
        .iter()
        .filter(|e| e.get("event_type").and_then(|v| v.as_str()) == Some(event_type))
        .collect()
}

pub fn evaluate_contract(contract: &Contract, events: &[Value]) -> ContractOutcome {
    let mut violated = Vec::new();
    for req in &contract.required {
        if !check_predicate(&contract.id, &req.id, events) {
            violated.push(req.id.clone());
        }
    }
    for f in &contract.forbidden {
        if check_predicate(&contract.id, &f.id, events) {
            violated.push(f.id.clone());
        }
    }
    if violated.is_empty() {
        ContractOutcome::Pass
    } else {
        ContractOutcome::Fail { violated }
    }
}

fn check_predicate(contract_id: &str, predicate_id: &str, events: &[Value]) -> bool {
    match (contract_id, predicate_id) {
        ("verify_after_final_change", "verifier_invoked") => {
            !events_of_type(events, "verifier.start").is_empty()
        }
        ("verify_after_final_change", "verifier_after_final_mutation") => {
            let last_mutation = events_of_type(events, "mutation").iter().map(|e| seq_of(e)).max();
            match last_mutation {
                None => !events_of_type(events, "verifier.start").is_empty(),
                Some(m) => events_of_type(events, "verifier.start")
                    .iter()
                    .any(|e| seq_of(e) > m),
            }
        }
        ("verify_after_final_change", "verifier_passed") => events_of_type(events, "verifier.result")
            .iter()
            .any(|e| e.get("passed").and_then(|v| v.as_bool()) == Some(true)),
        ("verify_after_final_change", "verifier_result_observed") => {
            let first_result = events_of_type(events, "verifier.result")
                .iter()
                .map(|e| seq_of(e))
                .min();
            match first_result {
                None => false,
                Some(v) => events_of_type(events, "assistant.claim")
                    .iter()
                    .any(|e| seq_of(e) > v),
            }
        }
        ("verify_after_final_change", "stale_verification") => {
            let claims = events_of_type(events, "assistant.claim");
            let results = events_of_type(events, "verifier.result");
            let mutations = events_of_type(events, "mutation");
            claims.iter().any(|c| {
                let c_seq = seq_of(c);
                results.iter().any(|r| {
                    let r_seq = seq_of(r);
                    r_seq < c_seq
                        && mutations
                            .iter()
                            .any(|m| seq_of(m) > r_seq && seq_of(m) < c_seq)
                    })
            })
        }
        ("no_success_before_evidence", "completion_after_evidence") => {
            let evidence_seq = events
                .iter()
                .filter(|e| {
                    matches!(
                        e.get("event_type").and_then(|v| v.as_str()),
                        Some("verifier.result") | Some("tool.result")
                    )
                })
                .map(seq_of)
                .min();
            match evidence_seq {
                None => false,
                Some(ev) => events_of_type(events, "assistant.claim")
                    .iter()
                    .any(|e| seq_of(e) > ev),
            }
        }
        ("no_success_before_evidence", "premature_success_claim") => {
            let first_evidence = events
                .iter()
                .filter(|e| {
                    matches!(
                        e.get("event_type").and_then(|v| v.as_str()),
                        Some("verifier.result") | Some("tool.result")
                    )
                })
                .map(seq_of)
                .min();
            events_of_type(events, "assistant.claim").iter().any(|c| match first_evidence {
                None => true,
                Some(ev) => seq_of(c) < ev,
            })
        }
        _ => false,
    }
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

#[cfg(test)]
mod predicate_tests {
    use super::*;
    use serde_json::json;

    fn contract_fixture() -> Contract {
        Contract {
            schema_version: "quecto.contract/v1".into(),
            id: "verify_after_final_change".into(),
            version: "1.0.0".into(),
            criticality: "critical".into(),
            applies_when: Default::default(),
            required: vec![
                PredicateRef { id: "verifier_invoked".into(), critical: false },
                PredicateRef { id: "verifier_after_final_mutation".into(), critical: false },
                PredicateRef { id: "verifier_passed".into(), critical: false },
                PredicateRef { id: "verifier_result_observed".into(), critical: false },
            ],
            forbidden: vec![PredicateRef { id: "stale_verification".into(), critical: true }],
            compatibility: CompatibilityConfig {
                reference_reliability_floor: 0.90,
                negative_flip_tolerance: 0.05,
            },
        }
    }

    #[test]
    fn passes_when_verify_happens_after_final_mutation_and_before_claim() {
        let events = vec![
            json!({"event_type": "run.start", "seq": 0}),
            json!({"event_type": "mutation", "seq": 1, "path": "a.txt"}),
            json!({"event_type": "verifier.start", "seq": 2}),
            json!({"event_type": "verifier.result", "seq": 3, "passed": true}),
            json!({"event_type": "assistant.claim", "seq": 4}),
        ];
        assert_eq!(evaluate_contract(&contract_fixture(), &events), ContractOutcome::Pass);
    }

    #[test]
    fn fails_with_stale_verification_when_mutation_follows_verifier_result() {
        let events = vec![
            json!({"event_type": "verifier.start", "seq": 0}),
            json!({"event_type": "verifier.result", "seq": 1, "passed": true}),
            json!({"event_type": "mutation", "seq": 2, "path": "a.txt"}),
            json!({"event_type": "assistant.claim", "seq": 3}),
        ];
        let outcome = evaluate_contract(&contract_fixture(), &events);
        match outcome {
            ContractOutcome::Fail { violated } => {
                assert!(violated.contains(&"stale_verification".to_string()));
                assert!(violated.contains(&"verifier_after_final_mutation".to_string()));
            }
            other => panic!("expected Fail, got {other:?}"),
        }
    }

    #[test]
    fn fails_when_verifier_never_invoked() {
        let events = vec![
            json!({"event_type": "mutation", "seq": 0, "path": "a.txt"}),
            json!({"event_type": "assistant.claim", "seq": 1}),
        ];
        let outcome = evaluate_contract(&contract_fixture(), &events);
        match outcome {
            ContractOutcome::Fail { violated } => {
                assert!(violated.contains(&"verifier_invoked".to_string()));
            }
            other => panic!("expected Fail, got {other:?}"),
        }
    }

    fn no_success_before_evidence_fixture() -> Contract {
        Contract {
            schema_version: "quecto.contract/v1".into(),
            id: "no_success_before_evidence".into(),
            version: "1.0.0".into(),
            criticality: "critical".into(),
            applies_when: Default::default(),
            required: vec![PredicateRef { id: "completion_after_evidence".into(), critical: false }],
            forbidden: vec![PredicateRef { id: "premature_success_claim".into(), critical: true }],
            compatibility: CompatibilityConfig {
                reference_reliability_floor: 0.90,
                negative_flip_tolerance: 0.05,
            },
        }
    }

    #[test]
    fn passes_when_claim_follows_tool_result_evidence() {
        let events = vec![
            json!({"event_type": "tool.call", "seq": 0, "tool_name": "read_file"}),
            json!({"event_type": "tool.result", "seq": 1, "tool_name": "read_file", "success": true}),
            json!({"event_type": "assistant.claim", "seq": 2}),
        ];
        assert_eq!(
            evaluate_contract(&no_success_before_evidence_fixture(), &events),
            ContractOutcome::Pass
        );
    }

    #[test]
    fn fails_when_claim_precedes_any_evidence() {
        let events = vec![
            json!({"event_type": "run.start", "seq": 0}),
            json!({"event_type": "assistant.claim", "seq": 1}),
            json!({"event_type": "tool.result", "seq": 2, "tool_name": "read_file", "success": true}),
        ];
        let outcome = evaluate_contract(&no_success_before_evidence_fixture(), &events);
        match outcome {
            ContractOutcome::Fail { violated } => {
                assert!(violated.contains(&"premature_success_claim".to_string()));
                assert!(violated.contains(&"completion_after_evidence".to_string()));
            }
            other => panic!("expected Fail, got {other:?}"),
        }
    }
}
