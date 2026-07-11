use crate::model::ToolCall;
use serde_json::Value;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Decision {
    Allow,
    Ask,
    Deny(String),
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Policy;

impl Policy {
    pub fn decide(&self, call: &ToolCall) -> Decision {
        match call.name.as_str() {
            "read_file" | "list_files" | "search_text" | "git_diff" | "git_status" => {
                Decision::Allow
            }
            "write_file" | "apply_patch" => Decision::Ask,
            "run_command" => {
                let command = call
                    .arguments
                    .get("command")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                deny_reason(command)
                    .map(Decision::Deny)
                    .unwrap_or(Decision::Ask)
            }
            _ => Decision::Deny(format!(
                "tool '{}' is not permitted by the built-in policy",
                call.name
            )),
        }
    }
}

fn deny_reason(command: &str) -> Option<String> {
    let normalized = command.to_ascii_lowercase();
    let words: Vec<&str> = normalized.split_whitespace().collect();
    let root_rm = words.first() == Some(&"rm")
        && words.iter().any(|w| *w == "/" || w.starts_with("/../"))
        && words
            .iter()
            .any(|w| w.starts_with('-') && w.contains('r') && w.contains('f'));
    let forbidden = words.first() == Some(&"sudo")
        || root_rm
        || words.iter().any(|w| w.starts_with("mkfs"))
        || words.first() == Some(&"fdisk")
        || (normalized.contains("diskutil") && normalized.contains("erasedisk"))
        || (words.first() == Some(&"git") && words.get(1) == Some(&"push"))
        || ["> /", ">/", ">> /", ">>/"]
            .iter()
            .any(|p| normalized.contains(p));
    forbidden.then(|| "command matches the hard denylist".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn call(name: &str, arguments: Value) -> ToolCall {
        ToolCall {
            id: "1".into(),
            name: name.into(),
            arguments,
        }
    }

    #[test]
    fn reads_are_allowed_and_mutations_ask() {
        let p = Policy;
        assert!(matches!(
            p.decide(&call("read_file", json!({}))),
            Decision::Allow
        ));
        assert!(matches!(
            p.decide(&call("write_file", json!({}))),
            Decision::Ask
        ));
        assert!(matches!(
            p.decide(&call("apply_patch", json!({}))),
            Decision::Ask
        ));
        assert!(matches!(
            p.decide(&call("run_command", json!({"command":"cargo test"}))),
            Decision::Ask
        ));
    }

    #[test]
    fn unknown_and_dangerous_commands_are_denied() {
        let p = Policy;
        assert!(matches!(
            p.decide(&call("custom", json!({}))),
            Decision::Deny(_)
        ));
        for command in [
            "sudo true",
            "rm -rf /",
            "mkfs.ext4 /dev/sda",
            "fdisk /dev/sda",
            "diskutil eraseDisk APFS X disk2",
            "git push origin main",
            "echo x > /tmp/x",
        ] {
            assert!(
                matches!(
                    p.decide(&call("run_command", json!({"command":command}))),
                    Decision::Deny(_)
                ),
                "{command}"
            );
        }
    }
}
