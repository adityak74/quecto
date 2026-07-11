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
    let forbidden = normalized
        .split([';', '&', '|', '\n'])
        .any(segment_is_forbidden)
        || ["> /", ">/", ">> /", ">>/"]
            .iter()
            .any(|p| normalized.contains(p));
    forbidden.then(|| "command matches the hard denylist".to_string())
}

fn segment_is_forbidden(segment: &str) -> bool {
    let words: Vec<&str> = segment.split_whitespace().collect();
    let words = command_after_env(&words);
    let root_rm = words.first() == Some(&"rm")
        && words.iter().any(|w| *w == "/" || w.starts_with("/../"))
        && ['r', 'f'].iter().all(|flag| {
            words
                .iter()
                .any(|word| word.starts_with('-') && word.contains(*flag))
        });
    words.first() == Some(&"sudo")
        || root_rm
        || words.iter().any(|w| w.starts_with("mkfs"))
        || words.first() == Some(&"fdisk")
        || (words.contains(&"diskutil") && words.contains(&"erasedisk"))
        || git_subcommand(words) == Some("push")
}

fn command_after_env<'a>(words: &'a [&str]) -> &'a [&'a str] {
    if words.first() != Some(&"env") {
        return words;
    }
    let mut index = 1;
    while let Some(word) = words.get(index) {
        let takes_value = matches!(
            *word,
            "-u" | "--unset" | "-c" | "--chdir" | "--argv0" | "-s" | "--split-string"
        );
        if takes_value {
            index += 2;
        } else if word.starts_with('-') || word.contains('=') {
            index += 1;
        } else {
            break;
        }
    }
    &words[index.min(words.len())..]
}

fn git_subcommand<'a>(words: &'a [&'a str]) -> Option<&'a str> {
    if words.first() != Some(&"git") {
        return None;
    }
    let mut index = 1;
    while let Some(word) = words.get(index) {
        if matches!(
            *word,
            "-c" | "--git-dir" | "--work-tree" | "--namespace" | "--super-prefix"
        ) {
            index += 2;
        } else if word.starts_with('-') {
            index += 1;
        } else {
            return Some(word);
        }
    }
    None
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

    #[test]
    fn compound_wrapped_and_split_flag_commands_are_denied() {
        let p = Policy;
        for command in [
            "echo ok; sudo true",
            "env git push origin main",
            "env FOO=bar git push origin main",
            "git -C repo push",
            "cd /tmp && fdisk /dev/sda",
            "rm -r -f /",
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
