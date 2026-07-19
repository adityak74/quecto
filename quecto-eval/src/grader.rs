use async_trait::async_trait;
use std::path::PathBuf;
use tokio::process::Command;

#[derive(Clone)]
pub struct Transcript {
    pub turns: u32,
    pub tokens: u32,
    pub latency_ms: u64,
}

pub struct EvalContext {
    pub workspace_path: PathBuf,
    pub transcript: Option<Transcript>,
}

pub struct GraderResult {
    pub passed: bool,
    pub reason: String,
}

#[async_trait]
pub trait Grader: Send + Sync {
    async fn evaluate(&self, ctx: &EvalContext) -> anyhow::Result<GraderResult>;
}

pub struct ScriptGrader {
    pub command: String,
}

#[async_trait]
impl Grader for ScriptGrader {
    async fn evaluate(&self, ctx: &EvalContext) -> anyhow::Result<GraderResult> {
        let trimmed = self.command.trim();
        if trimmed.is_empty() {
            return Ok(GraderResult { passed: false, reason: "Empty command".to_string() });
        }
        
        let output_result = Command::new("sh")
            .arg("-c")
            .arg(trimmed)
            .current_dir(&ctx.workspace_path)
            .output()
            .await;
            
        match output_result {
            Ok(output) => {
                let passed = output.status.success();
                Ok(GraderResult {
                    passed,
                    reason: format!("Exit code: {}", output.status),
                })
            }
            Err(e) => {
                Ok(GraderResult {
                    passed: false,
                    reason: format!("Failed to execute command: {}", e),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    
    #[tokio::test]
    async fn test_script_grader() {
        let dir = tempfile::tempdir().unwrap();
        let script_path = dir.path().join("verify.sh");
        fs::write(&script_path, "#!/bin/sh\nexit 0").unwrap();
        
        let grader = ScriptGrader { command: "sh verify.sh".to_string() };
        let ctx = EvalContext { workspace_path: dir.path().to_path_buf(), transcript: None };
        
        let result = grader.evaluate(&ctx).await.unwrap();
        assert!(result.passed);
    }

    #[tokio::test]
    async fn test_script_grader_nonzero_exit() {
        let dir = tempfile::tempdir().unwrap();
        let grader = ScriptGrader { command: "false".to_string() };
        let ctx = EvalContext { workspace_path: dir.path().to_path_buf(), transcript: None };
        
        let result = grader.evaluate(&ctx).await.unwrap();
        assert!(!result.passed);
        assert!(result.reason.contains("Exit code:"));
    }
    
    #[tokio::test]
    async fn test_script_grader_empty_command() {
        let dir = tempfile::tempdir().unwrap();
        let grader = ScriptGrader { command: "   ".to_string() };
        let ctx = EvalContext { workspace_path: dir.path().to_path_buf(), transcript: None };
        
        let result = grader.evaluate(&ctx).await.unwrap();
        assert!(!result.passed);
        assert_eq!(result.reason, "Empty command");
    }

    #[tokio::test]
    async fn test_script_grader_failed_to_spawn() {
        let grader = ScriptGrader { command: "echo hello".to_string() };
        let ctx = EvalContext { workspace_path: PathBuf::from("/dir_that_does_not_exist_quecto_test"), transcript: None };
        
        let result = grader.evaluate(&ctx).await.unwrap();
        assert!(!result.passed);
        assert!(result.reason.contains("Failed to execute command:"));
    }
}
