#[derive(Debug, PartialEq)]
pub enum ReasoningCommand {
    Show,
    Set(String),
}

/// A parsed line of chat input: a slash-command or plain text to send.
#[derive(Debug, PartialEq)]
pub enum ChatCommand {
    Help,
    Model,
    Context,
    Diff,
    Status,
    Undo,
    Approve,
    Deny,
    Clear,
    Exit,
    Tools,
    Reasoning(ReasoningCommand),
    Capsules,
    LoadCapsule(String),
    UnloadCapsule(String),
    InvokeCapsule { name: String, prompt: Option<String> },
    Say(String),
    Unknown(String),
}

/// Parse one line of REPL input. A leading `/` marks a command (case-insensitive,
/// first word only); anything else — including an empty line — is `Say`.
/// `capsule_names` is the set of currently discoverable capsule names, checked
/// only after every reserved built-in name has been ruled out, so a capsule can
/// never shadow a built-in.
pub fn parse_command(line: &str, capsule_names: &[String]) -> ChatCommand {
    let trimmed = line.trim();
    let Some(rest) = trimmed.strip_prefix('/') else {
        return ChatCommand::Say(trimmed.to_string());
    };
    let mut split = rest.splitn(2, char::is_whitespace);
    let name = split.next().unwrap_or("");
    let remainder = split.next().unwrap_or("").trim();

    match name.to_ascii_lowercase().as_str() {
        "reasoning" => {
            let mut args = remainder.split_whitespace();
            match (args.next(), args.next()) {
                (None, None) => ChatCommand::Reasoning(ReasoningCommand::Show),
                (Some(value), None) => {
                    ChatCommand::Reasoning(ReasoningCommand::Set(value.to_string()))
                }
                _ => ChatCommand::Unknown("reasoning".to_string()),
            }
        }
        "help" | "h" | "?" => ChatCommand::Help,
        "model" => ChatCommand::Model,
        "context" => ChatCommand::Context,
        "diff" => ChatCommand::Diff,
        "status" => ChatCommand::Status,
        "undo" => ChatCommand::Undo,
        "approve" => ChatCommand::Approve,
        "deny" => ChatCommand::Deny,
        "clear" => ChatCommand::Clear,
        "exit" | "quit" | "q" => ChatCommand::Exit,
        "tools" | "commands" => ChatCommand::Tools,
        "capsules" => ChatCommand::Capsules,
        "load" => ChatCommand::LoadCapsule(remainder.to_string()),
        "unload" => ChatCommand::UnloadCapsule(remainder.to_string()),
        other => match capsule_names.iter().find(|n| n.eq_ignore_ascii_case(other)) {
            Some(matched) => ChatCommand::InvokeCapsule {
                name: matched.clone(),
                prompt: (!remainder.is_empty()).then(|| remainder.to_string()),
            },
            None => ChatCommand::Unknown(other.to_string()),
        },
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_say_trimmed() {
        assert_eq!(
            parse_command("  fix the bug  ", &[]),
            ChatCommand::Say("fix the bug".to_string())
        );
    }

    #[test]
    fn known_commands_parse_case_insensitively() {
        assert_eq!(parse_command("/HELP", &[]), ChatCommand::Help);
        assert_eq!(parse_command("/Exit", &[]), ChatCommand::Exit);
        assert_eq!(parse_command("/diff", &[]), ChatCommand::Diff);
        assert_eq!(parse_command("/undo", &[]), ChatCommand::Undo);
        assert_eq!(parse_command("/approve", &[]), ChatCommand::Approve);
        assert_eq!(parse_command("/tools", &[]), ChatCommand::Tools);
        assert_eq!(parse_command("/deny", &[]), ChatCommand::Deny);
    }

    #[test]
    fn command_ignores_trailing_arguments() {
        assert_eq!(parse_command("/model gpt-4o", &[]), ChatCommand::Model);
    }

    #[test]
    fn unknown_slash_command_is_reported() {
        assert_eq!(
            parse_command("/frobnicate", &[]),
            ChatCommand::Unknown("frobnicate".to_string())
        );
    }

    #[test]
    fn aliases_map_to_canonical_commands() {
        assert_eq!(parse_command("/q", &[]), ChatCommand::Exit);
        assert_eq!(parse_command("/?", &[]), ChatCommand::Help);
        assert_eq!(parse_command("/commands", &[]), ChatCommand::Tools);
    }

    #[test]
    fn reasoning_without_argument_parses_as_show() {
        assert_eq!(
            parse_command("/reasoning", &[]),
            ChatCommand::Reasoning(ReasoningCommand::Show)
        );
    }

    #[test]
    fn reasoning_with_value_parses_as_set() {
        assert_eq!(
            parse_command("/reasoning high", &[]),
            ChatCommand::Reasoning(ReasoningCommand::Set("high".to_string()))
        );
    }

    #[test]
    fn reasoning_rejects_extra_arguments() {
        assert_eq!(
            parse_command("/reasoning high extra", &[]),
            ChatCommand::Unknown("reasoning".to_string())
        );
    }

    #[test]
    fn capsules_command_parses() {
        assert_eq!(parse_command("/capsules", &[]), ChatCommand::Capsules);
    }

    #[test]
    fn load_capsule_parses_name() {
        assert_eq!(
            parse_command("/load foo", &[]),
            ChatCommand::LoadCapsule("foo".to_string())
        );
    }

    #[test]
    fn load_without_name_is_empty_string() {
        assert_eq!(
            parse_command("/load", &[]),
            ChatCommand::LoadCapsule(String::new())
        );
    }

    #[test]
    fn unload_capsule_parses_name() {
        assert_eq!(
            parse_command("/unload foo", &[]),
            ChatCommand::UnloadCapsule("foo".to_string())
        );
    }

    #[test]
    fn capsule_name_invokes_with_no_prompt() {
        let names = vec!["foo".to_string()];
        assert_eq!(
            parse_command("/foo", &names),
            ChatCommand::InvokeCapsule {
                name: "foo".to_string(),
                prompt: None
            }
        );
    }

    #[test]
    fn capsule_name_invokes_with_prompt_text() {
        let names = vec!["foo".to_string()];
        assert_eq!(
            parse_command("/foo do the thing", &names),
            ChatCommand::InvokeCapsule {
                name: "foo".to_string(),
                prompt: Some("do the thing".to_string()),
            }
        );
    }

    #[test]
    fn capsule_name_matches_case_insensitively_and_returns_canonical_case() {
        let names = vec!["Foo".to_string()];
        assert_eq!(
            parse_command("/foo", &names),
            ChatCommand::InvokeCapsule {
                name: "Foo".to_string(),
                prompt: None
            }
        );
    }

    #[test]
    fn builtin_name_always_wins_over_a_same_named_capsule() {
        let names = vec!["model".to_string()];
        assert_eq!(parse_command("/model", &names), ChatCommand::Model);
    }

    #[test]
    fn unknown_name_with_no_capsule_match_is_still_unknown() {
        assert_eq!(
            parse_command("/frobnicate", &["foo".to_string()]),
            ChatCommand::Unknown("frobnicate".to_string())
        );
    }
}
