#[cfg(test)]
mod tests {
    use crate::config::EvalConfig;
    
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
telemetry_thresholds:
  max_turns: 10
"#;
        let config: EvalConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.id, "tb_01");
        assert_eq!(config.suite, "regression");
        assert_eq!(config.telemetry_thresholds.as_ref().unwrap().max_turns, Some(10));
    }
}
