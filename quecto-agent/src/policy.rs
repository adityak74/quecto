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
    let forbidden = tokenize_command(&normalized)
        .map(|segments| segments.iter().any(|words| segment_is_forbidden(words)))
        .unwrap_or(true)
        || normalized.contains('$')
        || normalized.contains("$(")
        || normalized.contains('`')
        || ["> /", ">/", ">> /", ">>/"]
            .iter()
            .any(|p| normalized.contains(p));
    forbidden.then(|| "command matches the hard denylist".to_string())
}

fn segment_is_forbidden(words: &[String]) -> bool {
    let Some(words) = unwrap_common_wrappers(words) else {
        return true;
    };
    if words.is_empty() {
        return false;
    }
    let executable = executable_name(&words[0]);
    if executable == "eval" {
        return true;
    }
    if matches!(executable, "sh" | "bash" | "zsh") {
        if let Some(index) = words
            .iter()
            .position(|word| word == "-c" || (word.starts_with('-') && word[1..].contains('c')))
        {
            let Some(payload) = words.get(index + 1) else {
                return true;
            };
            return deny_reason(payload).is_some();
        }
    }
    let root_rm = executable == "rm"
        && words.iter().any(|w| w == "/" || w.starts_with("/../"))
        && ['r', 'f'].iter().all(|flag| {
            words
                .iter()
                .any(|word| word.starts_with('-') && word.contains(*flag))
        });
    executable == "sudo"
        || root_rm
        || executable.starts_with("mkfs")
        || executable == "fdisk"
        || (executable == "diskutil" && words.iter().any(|word| word == "erasedisk"))
        || git_subcommand(words) == Some("push")
}

fn executable_name(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

fn unwrap_execution_wrappers(mut words: &[String]) -> Option<&[String]> {
    while let Some(name) = words.first().map(|word| executable_name(word)) {
        if name == "exec" {
            let mut index = 1;
            while let Some(option) = words.get(index).map(String::as_str) {
                match option {
                    "--" => {
                        index += 1;
                        break;
                    }
                    "-a" => {
                        words.get(index + 1)?;
                        index += 2;
                    }
                    "-c" | "-l" => index += 1,
                    option if option.starts_with('-') => return None,
                    _ => break,
                }
            }
            words = &words[index.min(words.len())..];
            continue;
        }
        if name != "command" {
            break;
        }
        let mut index = 1;
        while words.get(index).is_some_and(|word| word.starts_with('-')) {
            index += 1;
        }
        words = &words[index.min(words.len())..];
    }
    Some(words)
}

fn unwrap_common_wrappers(mut words: &[String]) -> Option<&[String]> {
    loop {
        let previous_len = words.len();
        while words.first().is_some_and(|word| is_assignment(word)) {
            words = &words[1..];
        }
        words = unwrap_execution_wrappers(words)?;
        words = command_after_env(words);
        if words.len() == previous_len {
            return Some(words);
        }
    }
}

fn is_assignment(word: &str) -> bool {
    let Some((name, _)) = word.split_once('=') else {
        return false;
    };
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|ch| ch == '_' || ch.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn command_after_env(words: &[String]) -> &[String] {
    if words.first().map(|word| executable_name(word)) != Some("env") {
        return words;
    }
    let mut index = 1;
    while let Some(word) = words.get(index) {
        let takes_value = matches!(
            word.as_str(),
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

fn git_subcommand(words: &[String]) -> Option<&str> {
    if words.first().map(|word| executable_name(word)) != Some("git") {
        return None;
    }
    let mut index = 1;
    while let Some(word) = words.get(index) {
        if matches!(
            word.as_str(),
            "-c" | "--git-dir" | "--work-tree" | "--namespace" | "--super-prefix"
        ) {
            index += 2;
        } else if word.starts_with('-') {
            index += 1;
        } else {
            return Some(word.as_str());
        }
    }
    None
}

fn tokenize_command(command: &str) -> Result<Vec<Vec<String>>, ()> {
    let mut segments = vec![Vec::new()];
    let mut word = String::new();
    let mut quote = None;
    let mut escaped = false;
    for ch in command.chars() {
        if escaped {
            word.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' && quote != Some('\'') {
            escaped = true;
            continue;
        }
        if let Some(active) = quote {
            if ch == active {
                quote = None;
            } else {
                word.push(ch);
            }
            continue;
        }
        if matches!(ch, '\'' | '"') {
            quote = Some(ch);
        } else if ch.is_whitespace() || matches!(ch, ';' | '&' | '|') {
            if !word.is_empty() {
                segments.last_mut().unwrap().push(std::mem::take(&mut word));
            }
            if matches!(ch, ';' | '&' | '|' | '\n') && !segments.last().unwrap().is_empty() {
                segments.push(Vec::new());
            }
        } else {
            word.push(ch);
        }
    }
    if escaped || quote.is_some() {
        return Err(());
    }
    if !word.is_empty() {
        segments.last_mut().unwrap().push(word);
    }
    segments.retain(|segment| !segment.is_empty());
    Ok(segments)
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

    #[test]
    fn path_qualified_and_common_wrapper_bypasses_are_denied() {
        let p = Policy;
        for command in [
            "/usr/bin/sudo true",
            "/usr/bin/git push origin main",
            "command sudo true",
            "command /usr/bin/git push origin main",
            "command env FOO=bar /usr/bin/git push origin main",
            "sh -c 'git push origin main'",
            "/bin/bash -c \"sudo true\"",
            "zsh -c 'echo ok; git push origin main'",
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
    fn ambiguous_shell_wrappers_are_denied_conservatively() {
        let p = Policy;
        for command in [
            "sh -c",
            "bash -c 'git push origin main",
            "command -- sudo true",
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
    fn assignment_variable_exec_and_eval_bypasses_are_denied() {
        let p = Policy;
        for command in [
            "FOO=1 /usr/bin/sudo true",
            "FOO=1 /usr/bin/git push origin main",
            "cmd=/usr/bin/sudo; $cmd true",
            "$VAR",
            "${VAR}",
            "sh -c \"$CMD\"",
            "exec sudo true",
            "/usr/bin/exec /usr/bin/git push origin main",
            "eval 'git push origin main'",
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
    fn benign_leading_assignment_remains_approval_gated() {
        let p = Policy;
        assert!(matches!(
            p.decide(&call(
                "run_command",
                json!({"command":"RUST_LOG=debug cargo test"})
            )),
            Decision::Ask
        ));
    }

    #[test]
    fn exec_argv0_option_cannot_hide_dangerous_executable() {
        let p = Policy;
        for command in [
            "exec -a harmless sudo true",
            "exec -a harmless /usr/bin/git push origin main",
        ] {
            assert!(
                matches!(
                    p.decide(&call("run_command", json!({"command":command}))),
                    Decision::Deny(_)
                ),
                "{command}"
            );
        }
        assert!(matches!(
            p.decide(&call(
                "run_command",
                json!({"command":"exec -a cargo cargo test"})
            )),
            Decision::Ask
        ));
    }
}
