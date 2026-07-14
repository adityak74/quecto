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
    Say(String),
    Unknown(String),
}

/// Parse one line of REPL input. A leading `/` marks a command (case-insensitive,
/// first word only); anything else — including an empty line — is `Say`.
pub fn parse_command(line: &str) -> ChatCommand {
    let trimmed = line.trim();
    let Some(rest) = trimmed.strip_prefix('/') else {
        return ChatCommand::Say(trimmed.to_string());
    };
    let name = rest.split_whitespace().next().unwrap_or("");
    match name.to_ascii_lowercase().as_str() {
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
        "tools" => ChatCommand::Tools,
        other => ChatCommand::Unknown(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_say_trimmed() {
        assert_eq!(
            parse_command("  fix the bug  "),
            ChatCommand::Say("fix the bug".to_string())
        );
    }

    #[test]
    fn known_commands_parse_case_insensitively() {
        assert_eq!(parse_command("/HELP"), ChatCommand::Help);
        assert_eq!(parse_command("/Exit"), ChatCommand::Exit);
        assert_eq!(parse_command("/diff"), ChatCommand::Diff);
        assert_eq!(parse_command("/undo"), ChatCommand::Undo);
        assert_eq!(parse_command("/approve"), ChatCommand::Approve);
        assert_eq!(parse_command("/tools"), ChatCommand::Tools);
        assert_eq!(parse_command("/deny"), ChatCommand::Deny);
    }

    #[test]
    fn command_ignores_trailing_arguments() {
        assert_eq!(parse_command("/model gpt-4o"), ChatCommand::Model);
    }

    #[test]
    fn unknown_slash_command_is_reported() {
        assert_eq!(
            parse_command("/frobnicate"),
            ChatCommand::Unknown("frobnicate".to_string())
        );
    }

    #[test]
    fn aliases_map_to_canonical_commands() {
        assert_eq!(parse_command("/q"), ChatCommand::Exit);
        assert_eq!(parse_command("/?"), ChatCommand::Help);
    }
}
