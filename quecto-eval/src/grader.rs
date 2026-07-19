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
        let parts: Vec<&str> = self.command.split_whitespace().collect();
        if parts.is_empty() {
            return Ok(GraderResult { passed: false, reason: "Empty command".to_string() });
        }
        
        let output = Command::new(parts[0])
            .args(&parts[1..])
            .current_dir(&ctx.workspace_path)
            .output()
            .await?;
            
        let passed = output.status.success();
        Ok(GraderResult {
            passed,
            reason: format!("Exit code: {}", output.status),
        })
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
}
