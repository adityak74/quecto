//! Analyzer: episodes.jsonl -> report.md, results.csv, cost-vs-success.svg.

use crate::ablate::Rng;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

struct Ep {
    arm: String,
    task: String,
    passed: bool,
    false_completion: bool,
    tokens: f64,
    latency_s: f64,
    tool_calls: f64,
    failure: String,
    infra: bool,
}

fn load(path: &Path) -> anyhow::Result<Vec<Ep>> {
    let mut eps = Vec::new();
    for line in fs::read_to_string(path)?.lines() {
        let v: Value = serde_json::from_str(line)?;
        let f = |p: &str| v.pointer(p).and_then(Value::as_f64).unwrap_or(0.0);
        let failure = v["failure_class"].as_str().unwrap_or("").to_string();
        eps.push(Ep {
            arm: v.pointer("/configuration/arm").and_then(Value::as_str).unwrap_or("?").into(),
            task: v.pointer("/task/id").and_then(Value::as_str).unwrap_or("?").into(),
            passed: v.pointer("/outcome/passed") == Some(&Value::Bool(true)),
            false_completion: v.pointer("/outcome/false_completion") == Some(&Value::Bool(true)),
            tokens: f("/token_usage/total"),
            latency_s: f("/latency_ms") / 1000.0,
            tool_calls: f("/events/tool_calls"),
            infra: failure == "environment_or_infra",
            failure,
        });
    }
    Ok(eps)
}

/// Wilson 95% interval for a proportion.
pub fn wilson(k: f64, n: f64) -> (f64, f64) {
    if n == 0.0 {
        return (0.0, 0.0);
    }
    let z = 1.96;
    let p = k / n;
    let d = 1.0 + z * z / n;
    let c = p + z * z / (2.0 * n);
    let m = z * ((p * (1.0 - p) + z * z / (4.0 * n)) / n).sqrt();
    ((c - m) / d, (c + m) / d)
}

/// Bootstrap 95% CI of the mean of paired per-task differences.
pub fn bootstrap_mean_ci(diffs: &[f64], seed: u64) -> (f64, f64, f64) {
    if diffs.is_empty() {
        return (0.0, 0.0, 0.0);
    }
    let mean = diffs.iter().sum::<f64>() / diffs.len() as f64;
    let mut rng = Rng(seed.max(1));
    let mut means: Vec<f64> = (0..2000)
        .map(|_| {
            (0..diffs.len()).map(|_| diffs[(rng.next() % diffs.len() as u64) as usize]).sum::<f64>()
                / diffs.len() as f64
        })
        .collect();
    means.sort_by(|a, b| a.partial_cmp(b).unwrap());
    (mean, means[50], means[1949])
}

fn pass_rate_by_task(eps: &[&Ep]) -> BTreeMap<String, f64> {
    let mut m: BTreeMap<String, (f64, f64)> = BTreeMap::new();
    for e in eps {
        let x = m.entry(e.task.clone()).or_default();
        x.0 += e.passed as u8 as f64;
        x.1 += 1.0;
    }
    m.into_iter().map(|(k, (p, n))| (k, p / n)).collect()
}

pub fn report(episodes: &Path, out_dir: &Path) -> anyhow::Result<()> {
    let eps = load(episodes)?;
    fs::create_dir_all(out_dir)?;
    let mut arms: BTreeMap<String, Vec<&Ep>> = BTreeMap::new();
    for e in &eps {
        arms.entry(e.arm.clone()).or_default().push(e);
    }
    let mean = |v: &[&Ep], f: fn(&Ep) -> f64| v.iter().map(|e| f(e)).sum::<f64>() / v.len().max(1) as f64;

    let mut md = String::from("# Harness ablation report\n\n");
    writeln!(md, "{} episodes. Exploratory: small sample, intervals are wide; infrastructure errors are counted, not dropped.\n", eps.len())?;
    md.push_str("## Per-arm results\n\n| Arm | n | Pass | Rate | 95% CI (Wilson) | False completions | Infra errors | Mean tokens | Tokens / pass | Mean latency (s) | Mean tool calls |\n|---|---|---|---|---|---|---|---|---|---|---|\n");
    let mut csv = String::from("arm,n,passes,rate,ci_lo,ci_hi,false_completions,infra_errors,mean_tokens,tokens_per_pass,mean_latency_s,mean_tool_calls\n");
    let mut points = Vec::new();
    for (arm, v) in &arms {
        let n = v.len() as f64;
        let k = v.iter().filter(|e| e.passed).count() as f64;
        let (lo, hi) = wilson(k, n);
        let fc = v.iter().filter(|e| e.false_completion).count();
        let infra = v.iter().filter(|e| e.infra).count();
        let mt = mean(v, |e| e.tokens);
        let tpp = if k > 0.0 { v.iter().map(|e| e.tokens).sum::<f64>() / k } else { f64::NAN };
        let (ml, mc) = (mean(v, |e| e.latency_s), mean(v, |e| e.tool_calls));
        writeln!(md, "| {arm} | {n} | {k} | {:.2} | {lo:.2}–{hi:.2} | {fc} | {infra} | {mt:.0} | {tpp:.0} | {ml:.1} | {mc:.1} |", k / n)?;
        writeln!(csv, "{arm},{n},{k},{:.4},{lo:.4},{hi:.4},{fc},{infra},{mt:.1},{tpp:.1},{ml:.2},{mc:.2}", k / n)?;
        points.push((arm.clone(), mt, k / n));
    }

    if let Some(base) = arms.get("A") {
        md.push_str("\n## Paired comparison vs arm A (per-task pass-rate difference)\n\n| Arm | Tasks paired | Mean Δ pass rate | 95% bootstrap CI |\n|---|---|---|---|\n");
        let ba = pass_rate_by_task(base);
        for (arm, v) in arms.iter().filter(|(a, _)| *a != "A") {
            let br = pass_rate_by_task(v);
            let diffs: Vec<f64> = br.iter().filter_map(|(t, r)| ba.get(t).map(|a| r - a)).collect();
            let (m, lo, hi) = bootstrap_mean_ci(&diffs, 42);
            writeln!(md, "| {arm} | {} | {m:+.3} | {lo:+.3} to {hi:+.3} |", diffs.len())?;
        }
    }

    md.push_str("\n## Failure attribution\n\n| Failure class | ");
    md.push_str(&arms.keys().cloned().collect::<Vec<_>>().join(" | "));
    md.push_str(" |\n|---|");
    md.push_str(&"---|".repeat(arms.len()));
    md.push('\n');
    let classes: std::collections::BTreeSet<&str> =
        eps.iter().filter(|e| e.failure != "none").map(|e| e.failure.as_str()).collect();
    for c in classes {
        write!(md, "| {c} |")?;
        for v in arms.values() {
            write!(md, " {} |", v.iter().filter(|e| e.failure == c).count())?;
        }
        md.push('\n');
    }

    fs::write(out_dir.join("report.md"), md)?;
    fs::write(out_dir.join("results.csv"), csv)?;
    fs::write(out_dir.join("cost-vs-success.svg"), svg(&points))?;
    Ok(())
}

fn svg(points: &[(String, f64, f64)]) -> String {
    let max_x = points.iter().map(|p| p.1).fold(1.0, f64::max) * 1.1;
    let mut s = String::from("<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 440 320' font-family='sans-serif' font-size='12'><rect width='440' height='320' fill='white'/><line x1='50' y1='270' x2='420' y2='270' stroke='#333'/><line x1='50' y1='20' x2='50' y2='270' stroke='#333'/><text x='235' y='308' text-anchor='middle'>mean tokens per run</text><text x='14' y='150' transform='rotate(-90 14 150)' text-anchor='middle'>pass rate</text>");
    for (arm, x, y) in points {
        let (px, py) = (50.0 + 370.0 * x / max_x, 270.0 - 250.0 * y);
        let _ = write!(s, "<circle cx='{px:.1}' cy='{py:.1}' r='6' fill='#2563eb'/><text x='{:.1}' y='{:.1}'>{arm}</text>", px + 9.0, py + 4.0);
    }
    s.push_str("</svg>");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wilson_bounds_contain_estimate() {
        let (lo, hi) = wilson(8.0, 10.0);
        assert!(lo < 0.8 && 0.8 < hi && lo > 0.4 && hi < 0.97);
    }

    #[test]
    fn bootstrap_of_constant_is_that_constant() {
        let (m, lo, hi) = bootstrap_mean_ci(&[0.5, 0.5, 0.5], 1);
        assert_eq!((m, lo, hi), (0.5, 0.5, 0.5));
    }

    #[test]
    fn report_writes_outputs() {
        let dir = tempfile::tempdir().unwrap();
        let ep = |arm: &str, passed: bool| format!("{{\"configuration\":{{\"arm\":\"{arm}\"}},\"task\":{{\"id\":\"t1\"}},\"outcome\":{{\"passed\":{passed},\"false_completion\":false}},\"token_usage\":{{\"total\":100}},\"latency_ms\":2000,\"events\":{{\"tool_calls\":3}},\"failure_class\":\"{}\"}}\n", if passed {"none"} else {"incorrect_completion"});
        let p = dir.path().join("e.jsonl");
        fs::write(&p, ep("A", false) + &ep("B", true)).unwrap();
        report(&p, dir.path()).unwrap();
        let md = fs::read_to_string(dir.path().join("report.md")).unwrap();
        assert!(md.contains("| B | 1 |") && md.contains("incorrect_completion"));
    }
}
