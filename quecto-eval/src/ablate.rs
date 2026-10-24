//! Harness-ablation runner: executes tasks against named arms (flag/env
//! bundles from an arms TOML) and writes one JSONL episode per run.

use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const SCHEMA: &str = "quecto.episode/v0";

#[derive(Debug, Clone)]
pub struct Arm {
    pub name: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
}

pub struct Opts {
    pub arms_file: PathBuf,
    pub tasks_dir: PathBuf,
    pub out_dir: PathBuf,
    pub agent_binary: PathBuf,
    pub model: String,
    pub base_url: String,
    pub reasoning_fixed: String,
    pub arms: Vec<String>,
    pub tasks: Vec<String>,
    pub repeats: u32,
    pub seed: u64,
    pub timeout_secs: u64,
    pub max_steps: u32,
    /// Extra attempts when a run ends in an infrastructure error (all attempts are kept).
    pub retry_infra: u32,
}

#[derive(Debug, Clone)]
pub struct Task {
    pub id: String,
    pub dir: PathBuf,
    pub category: String,
    pub prompt: String,
    /// Step budget for the first stage of a resume task.
    pub interrupt_steps: Option<u32>,
}

pub fn load_arms(path: &Path, reasoning_fixed: &str) -> anyhow::Result<Vec<Arm>> {
    let doc: toml::Table = fs::read_to_string(path)?.parse()?;
    let table = doc
        .get("arm")
        .and_then(|v| v.as_table())
        .ok_or_else(|| anyhow::anyhow!("{}: missing [arm.*] tables", path.display()))?;
    let sub = |v: &str| {
        v.replace("$VERIFY_CMD", "bash verify.sh")
            .replace("$REASONING_FIXED", reasoning_fixed)
    };
    let mut arms = Vec::new();
    for (name, v) in table {
        let args = v
            .get("args")
            .and_then(|a| a.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str().map(sub)).collect())
            .unwrap_or_default();
        let env = v
            .get("env")
            .and_then(|e| e.as_table())
            .map(|e| {
                e.iter()
                    .filter_map(|(k, x)| x.as_str().map(|s| (k.clone(), sub(s))))
                    .collect()
            })
            .unwrap_or_default();
        arms.push(Arm {
            name: name.clone(),
            args,
            env,
        });
    }
    Ok(arms)
}

pub fn load_tasks(dir: &Path) -> anyhow::Result<Vec<Task>> {
    let mut tasks = Vec::new();
    for entry in fs::read_dir(dir)? {
        let p = entry?.path();
        let id = p.file_name().unwrap().to_string_lossy().to_string();
        if !p.is_dir() || id.starts_with('.') || !p.join("prompt.md").exists() {
            continue;
        }
        let read = |f: &str| {
            fs::read_to_string(p.join(f))
                .ok()
                .map(|s| s.trim().to_string())
        };
        tasks.push(Task {
            category: read("category").unwrap_or_else(|| "uncategorized".into()),
            prompt: fs::read_to_string(p.join("prompt.md"))?,
            interrupt_steps: read("interrupt_steps").and_then(|s| s.parse().ok()),
            dir: p,
            id,
        });
    }
    tasks.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(tasks)
}

/// Deterministic xorshift so shuffles and bootstraps are reproducible.
pub struct Rng(pub u64);
impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

fn copy_dir(src: &Path, dst: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(dst)?;
    for e in fs::read_dir(src)? {
        let e = e?;
        let to = dst.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_dir(&e.path(), &to)?;
        } else {
            fs::copy(e.path(), to)?;
        }
    }
    Ok(())
}

/// Run `cmd` with a wall-clock limit; returns (exit success, timed_out).
fn run_limited(mut cmd: Command, log: &Path, timeout: Duration) -> anyhow::Result<(bool, bool)> {
    let out = fs::OpenOptions::new().create(true).append(true).open(log)?;
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(out.try_clone()?)
        .stderr(out)
        .spawn()?;
    // `Instant` stops while the machine sleeps; also check wall-clock time so a
    // sleeping laptop cannot turn a timeout into an unbounded run.
    let start = Instant::now();
    let wall_start = std::time::SystemTime::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok((status.success(), false));
        }
        if start.elapsed() > timeout || wall_start.elapsed().is_ok_and(|e| e > timeout) {
            let _ = child.kill();
            let _ = child.wait();
            return Ok((false, true));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Counts and totals extracted from one agent trace file.
#[derive(Default, Debug)]
pub struct TraceSummary {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub turns: u64,
    pub tool_calls: u64,
    pub tool_failures: u64,
    pub verifier_runs: u64,
    pub verifier_failures: u64,
    pub claims: u64,
    pub infra_errors: u64,
    pub termination: Option<String>,
}

pub fn summarize_trace(path: &Path) -> TraceSummary {
    let mut s = TraceSummary::default();
    let Ok(text) = fs::read_to_string(path) else {
        return s;
    };
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let n = |k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0);
        match v["event_type"].as_str().unwrap_or("") {
            "turn" => {
                s.turns += 1;
                s.prompt_tokens += n("prompt_tokens");
                s.completion_tokens += n("completion_tokens");
            }
            "tool.call" => s.tool_calls += 1,
            "tool.result" if v["success"] == json!(false) => s.tool_failures += 1,
            "verifier.result" => {
                s.verifier_runs += 1;
                if v["passed"] == json!(false) {
                    s.verifier_failures += 1;
                }
            }
            "assistant.claim" => s.claims += 1,
            "infrastructure.error" => s.infra_errors += 1,
            "termination" => s.termination = v["reason"].as_str().map(String::from),
            _ => {}
        }
    }
    s
}

/// Deterministic failure attribution (see docs/research/failure-taxonomy.md).
pub fn classify(
    passed: bool,
    timed_out: bool,
    spawn_failed: bool,
    persistence_off_resume_task: bool,
    t: &TraceSummary,
) -> &'static str {
    if passed {
        return "none";
    }
    if spawn_failed || t.infra_errors > 0 || t.termination.is_none() && !timed_out {
        return "environment_or_infra";
    }
    if timed_out {
        return "budget_exhaustion";
    }
    match t.termination.as_deref() {
        Some("step_limit") => "budget_exhaustion",
        Some("verification_failed") => "verification_failure",
        Some("blocked") => "policy_block",
        Some("repeated_action") => "tool_execution_failure",
        Some("error") => "environment_or_infra",
        _ if persistence_off_resume_task => "context_or_state_loss",
        _ if t.tool_failures > 0 => "tool_execution_failure",
        _ => "incorrect_completion",
    }
}

fn git_head() -> String {
    Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

fn latest_session(db: &Path) -> Option<String> {
    let conn = rusqlite::Connection::open(db).ok()?;
    conn.query_row(
        "SELECT id FROM sessions ORDER BY created DESC LIMIT 1",
        [],
        |r| r.get(0),
    )
    .ok()
}

fn warm_up(base_url: &str, model: &str) {
    let body =
        json!({"model": model, "max_tokens": 1, "messages": [{"role": "user", "content": "hi"}]});
    let _ = Command::new("curl")
        .args([
            "-s",
            "-m",
            "300",
            "-o",
            "/dev/null",
            "-H",
            "content-type: application/json",
            "-d",
        ])
        .arg(body.to_string())
        .arg(format!(
            "{}/chat/completions",
            base_url.trim_end_matches('/')
        ))
        .status();
}

pub fn run(opts: &Opts) -> anyhow::Result<()> {
    let arms: Vec<Arm> = load_arms(&opts.arms_file, &opts.reasoning_fixed)?
        .into_iter()
        .filter(|a| opts.arms.is_empty() || opts.arms.contains(&a.name))
        .collect();
    let tasks: Vec<Task> = load_tasks(&opts.tasks_dir.canonicalize()?)?
        .into_iter()
        .filter(|t| opts.tasks.is_empty() || opts.tasks.contains(&t.id))
        .collect();
    anyhow::ensure!(
        !arms.is_empty() && !tasks.is_empty(),
        "no arms or tasks selected"
    );

    let out = opts.out_dir.canonicalize().or_else(|_| {
        fs::create_dir_all(&opts.out_dir)?;
        opts.out_dir.canonicalize()
    })?;
    for d in ["traces", "logs", "workspaces", "state"] {
        fs::create_dir_all(out.join(d))?;
    }
    let agent = opts.agent_binary.canonicalize()?;
    let episodes_path = out.join("episodes.jsonl");
    let done: HashSet<String> = fs::read_to_string(&episodes_path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter_map(|v| v["episode_id"].as_str().map(String::from))
        .collect();

    let mut plan = Vec::new();
    for t in &tasks {
        for a in &arms {
            for rep in 0..opts.repeats {
                plan.push((t.clone(), a.clone(), rep));
            }
        }
    }
    let mut rng = Rng(opts.seed.max(1));
    for i in (1..plan.len()).rev() {
        plan.swap(i, (rng.next() % (i as u64 + 1)) as usize);
    }

    let harness_version = format!(
        "{}@{}",
        Command::new(&agent)
            .arg("--version")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .unwrap_or_default()
            .trim(),
        git_head()
    );
    let experiment = out.file_name().unwrap().to_string_lossy().to_string();
    warm_up(&opts.base_url, &opts.model);
    println!("{} runs planned ({} already done)", plan.len(), done.len());

    for (idx, (task, arm, rep)) in plan.iter().enumerate() {
        let episode_id = format!("{}-{}-{}-{}", experiment, arm.name, task.id, rep);
        if done.contains(&episode_id) {
            continue;
        }
        println!("[{}/{}] {episode_id}", idx + 1, plan.len());
        for attempt in 1..=opts.retry_infra + 1 {
            let ws = out.join("workspaces").join(&episode_id);
            let _ = fs::remove_dir_all(&ws);
            fs::create_dir_all(&ws)?;
            // Setup runs from the pristine task dir; only the visible gate script
            // is copied in, and only for arms that enable verification.
            let setup = task.dir.join("setup.sh");
            if setup.exists() {
                let ok = Command::new("bash")
                    .arg(&setup)
                    .current_dir(&ws)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()?
                    .success();
                anyhow::ensure!(ok, "setup.sh failed for {}", task.id);
            }
            let gate = arm.env.contains_key("QUECTO_VERIFY");
            // gate.sh (if present) is the visible gate; it must not reveal the held-out answer.
            let gate_src = [task.dir.join("gate.sh"), task.dir.join("verify.sh")]
                .into_iter()
                .find(|p| p.exists());
            if let (true, Some(src)) = (gate, gate_src) {
                fs::copy(src, ws.join("verify.sh"))?;
            }
            let env_hash = crate::snapshot::snapshot_hash(&ws)?;
            let state_db = out.join("state").join(format!("{episode_id}.db"));
            let log = out.join("logs").join(format!("{episode_id}.log"));
            let timeout = Duration::from_secs(opts.timeout_secs);
            let persistence = !arm.args.iter().any(|a| a == "--no-session");

            let stage = |n: u32,
                         resume_id: Option<&str>,
                         steps: u32|
             -> anyhow::Result<(bool, bool, PathBuf)> {
                let trace = out.join("traces").join(format!("{episode_id}-s{n}.jsonl"));
                let mut cmd = Command::new(&agent);
                cmd.current_dir(&ws);
                // Flags must follow `resume <id>`: quecto-agent parses
                // `--yes resume <id>` as the task text "resume <id>".
                match resume_id {
                    Some(id) => cmd.arg("resume").arg(id).arg("--yes").args(&arm.args),
                    None => cmd.arg("--yes").args(&arm.args).arg(&task.prompt),
                };
                cmd.env("QUECTO_TRACE_FILE", &trace)
                    .env("QUECTO_STATE_DB", &state_db)
                    .env("QUECTO_EXPERIMENT_ID", &experiment)
                    .env("QUECTO_TASK_ID", &task.id)
                    .env("QUECTO_RUNTIME_ID", &arm.name)
                    .env("QUECTO_RUN_ID", &episode_id)
                    .env("QUECTO_REPETITION", rep.to_string())
                    .env("QUECTO_SNAPSHOT_HASH", &env_hash)
                    .env("QUECTO_COMMIT", git_head())
                    .env("QUECTO_MODEL", &opts.model)
                    .env("QUECTO_BASE_URL", &opts.base_url)
                    .env("QUECTO_MAX_STEPS", steps.to_string())
                    .envs(&arm.env);
                match run_limited(cmd, &log, timeout) {
                    Ok((ok, to)) => Ok((ok, to, trace)),
                    Err(_) => Ok((false, false, trace)),
                }
            };

            let start = Instant::now();
            let (steps1, staged) = match task.interrupt_steps {
                Some(n) => (n, true),
                None => (opts.max_steps, false),
            };
            let (_, mut timed_out, t1) = stage(1, None, steps1)?;
            let mut traces = vec![t1.clone()];
            let spawn_failed = !t1.exists();
            if staged
                && !timed_out
                && summarize_trace(&t1).termination.as_deref() != Some("complete")
            {
                let resume_id = if persistence {
                    latest_session(&state_db)
                } else {
                    None
                };
                let (_, to, t2) = stage(2, resume_id.as_deref(), opts.max_steps)?;
                timed_out = to;
                traces.push(t2);
            }
            let latency_ms = start.elapsed().as_millis() as u64;

            // Held-out grading: always the pristine verify.sh, run in the workspace.
            let verify = task.dir.join("verify.sh");
            let passed = verify.exists()
                && Command::new("bash")
                    .arg(&verify)
                    .current_dir(&ws)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()?
                    .success();

            let mut sum = TraceSummary::default();
            for t in &traces {
                let s = summarize_trace(t);
                sum.prompt_tokens += s.prompt_tokens;
                sum.completion_tokens += s.completion_tokens;
                sum.turns += s.turns;
                sum.tool_calls += s.tool_calls;
                sum.tool_failures += s.tool_failures;
                sum.verifier_runs += s.verifier_runs;
                sum.verifier_failures += s.verifier_failures;
                sum.claims += s.claims;
                sum.infra_errors += s.infra_errors;
                sum.termination = s.termination.or(sum.termination.take());
            }
            let resume_task_without_state = staged && !persistence;
            let failure = classify(
                passed,
                timed_out,
                spawn_failed,
                resume_task_without_state,
                &sum,
            );
            let episode = json!({
                "schema": SCHEMA,
                "episode_id": episode_id,
                "harness_version": harness_version,
                "model": {"id": opts.model, "base_url": opts.base_url, "reasoning_fixed": opts.reasoning_fixed},
                "task": {"id": task.id, "category": task.category, "environment_hash": env_hash, "resume_task": staged},
                "configuration": {"arm": arm.name, "args": arm.args, "env": arm.env},
                "budget": {"max_steps": opts.max_steps, "timeout_secs": opts.timeout_secs},
                "repetition": rep,
                "attempt": attempt,
                "outcome": {
                    "passed": passed,
                    "termination": sum.termination,
                    "timed_out": timed_out,
                    "false_completion": !passed && sum.termination.as_deref() == Some("complete"),
                },
                "events": {
                    "turns": sum.turns, "tool_calls": sum.tool_calls, "tool_failures": sum.tool_failures,
                    "verifier_runs": sum.verifier_runs, "verifier_failures": sum.verifier_failures,
                    "claims": sum.claims, "infra_errors": sum.infra_errors,
                },
                "token_usage": {"prompt": sum.prompt_tokens, "completion": sum.completion_tokens,
                                "total": sum.prompt_tokens + sum.completion_tokens},
                "latency_ms": latency_ms,
                "failure_class": failure,
                "artifacts": {"traces": traces.iter().map(|t| t.file_name().unwrap().to_string_lossy()).collect::<Vec<_>>()},
            });
            let mut f = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&episodes_path)?;
            writeln!(f, "{episode}")?;
            let _ = fs::remove_dir_all(&ws);
            if failure != "environment_or_infra" {
                break;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arms_substitute_placeholders() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("arms.toml");
        fs::write(&p, "[arm.F]\nargs=[]\nenv={ QUECTO_REASONING_MODE = \"$REASONING_FIXED\", QUECTO_VERIFY = \"$VERIFY_CMD\" }\n").unwrap();
        let arms = load_arms(&p, "high").unwrap();
        assert_eq!(arms[0].env["QUECTO_REASONING_MODE"], "high");
        assert_eq!(arms[0].env["QUECTO_VERIFY"], "bash verify.sh");
    }

    #[test]
    fn classify_covers_main_branches() {
        let mut t = TraceSummary {
            termination: Some("complete".into()),
            ..Default::default()
        };
        assert_eq!(classify(true, false, false, false, &t), "none");
        assert_eq!(
            classify(false, false, false, false, &t),
            "incorrect_completion"
        );
        assert_eq!(
            classify(false, false, false, true, &t),
            "context_or_state_loss"
        );
        t.tool_failures = 1;
        assert_eq!(
            classify(false, false, false, false, &t),
            "tool_execution_failure"
        );
        t.termination = Some("step_limit".into());
        assert_eq!(
            classify(false, false, false, false, &t),
            "budget_exhaustion"
        );
        assert_eq!(classify(false, true, false, false, &t), "budget_exhaustion");
        assert_eq!(
            classify(false, false, true, false, &t),
            "environment_or_infra"
        );
    }

    #[test]
    fn summarize_trace_sums_tokens_and_reads_termination() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.jsonl");
        fs::write(&p, "{\"event_type\":\"turn\",\"prompt_tokens\":10,\"completion_tokens\":2}\n{\"event_type\":\"turn\",\"prompt_tokens\":20,\"completion_tokens\":3}\n{\"event_type\":\"tool.result\",\"success\":false}\n{\"event_type\":\"termination\",\"reason\":\"complete\"}\n").unwrap();
        let s = summarize_trace(&p);
        assert_eq!(
            (
                s.prompt_tokens,
                s.completion_tokens,
                s.turns,
                s.tool_failures
            ),
            (30, 5, 2, 1)
        );
        assert_eq!(s.termination.as_deref(), Some("complete"));
    }

    #[test]
    fn rng_is_deterministic() {
        let (mut a, mut b) = (Rng(7), Rng(7));
        assert_eq!(a.next(), b.next());
    }
}
