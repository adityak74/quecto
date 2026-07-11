use crate::tools::Context;

/// Outcome of running one verification command.
pub struct VerifyResult {
    pub command: String,
    pub passed: bool,
    pub output: String,
}

/// Aggregate of every verification command for one gate check.
pub struct VerifyReport {
    pub results: Vec<VerifyResult>,
}

impl VerifyReport {
    pub fn all_passed(&self) -> bool {
        self.results.iter().all(|r| r.passed)
    }

    /// A model-facing observation summarizing the failed checks.
    pub fn observation(&self) -> String {
        let mut out =
            String::from("Verification failed. Fix the reported problems and finish again.\n");
        for r in self.results.iter().filter(|r| !r.passed) {
            out.push_str(&format!("\n$ {}\n{}\n", r.command, r.output));
        }
        out
    }
}

/// Fixed (non-flavor) verification commands run as a completion gate. Commands
/// bypass the approval prompt but still execute inside the sandbox.
pub struct Verifier {
    commands: Vec<String>,
}

impl Verifier {
    pub fn new(commands: Vec<String>) -> Self {
        Verifier {
            commands: commands
                .into_iter()
                .map(|c| c.trim().to_string())
                .filter(|c| !c.is_empty())
                .collect(),
        }
    }

    /// Parse newline-separated commands from `QUECTO_VERIFY`. Returns `None`
    /// when unset or effectively empty.
    pub fn from_env() -> Option<Self> {
        let raw = std::env::var("QUECTO_VERIFY").ok()?;
        let v = Verifier::new(raw.lines().map(|l| l.to_string()).collect());
        if v.is_empty() {
            None
        } else {
            Some(v)
        }
    }

    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    /// Run every command through the sandbox; a non-zero (or signal) exit fails.
    pub fn run(&self, cx: &Context) -> VerifyReport {
        let results = self
            .commands
            .iter()
            .map(|command| match cx.run_verify(command) {
                Ok(out) => VerifyResult {
                    command: command.clone(),
                    passed: out.status == Some(0),
                    output: out.render(),
                },
                Err(e) => VerifyResult {
                    command: command.clone(),
                    passed: false,
                    output: format!("error: {}", e.message),
                },
            })
            .collect();
        VerifyReport { results }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::cancel_token;
    use tempfile::{tempdir, TempDir};

    fn cx() -> (Context, TempDir) {
        let dir = tempdir().unwrap();
        let cx = Context::new(dir.path().to_path_buf(), cancel_token());
        (cx, dir)
    }

    #[test]
    fn new_trims_and_drops_blank_commands() {
        let v = Verifier::new(vec!["  ".into(), "echo hi".into(), "".into()]);
        assert!(!v.is_empty());
        let (cx, _dir) = cx();
        let report = v.run(&cx);
        assert_eq!(report.results.len(), 1);
        assert_eq!(report.results[0].command, "echo hi");
    }

    #[test]
    fn all_passed_true_when_every_command_exits_zero() {
        let (cx, _dir) = cx();
        let report = Verifier::new(vec!["exit 0".into(), "true".into()]).run(&cx);
        assert!(report.all_passed());
    }

    #[test]
    fn failure_is_flagged_and_summarized() {
        let (cx, _dir) = cx();
        let report = Verifier::new(vec!["exit 0".into(), "exit 1".into()]).run(&cx);
        assert!(!report.all_passed());
        let obs = report.observation();
        assert!(obs.contains("Verification failed"));
        assert!(obs.contains("$ exit 1"));
        assert!(!obs.contains("$ exit 0"));
    }

    #[test]
    fn empty_verifier_is_reported_empty() {
        assert!(Verifier::new(vec![]).is_empty());
        assert!(Verifier::new(vec!["   ".into()]).is_empty());
    }
}
