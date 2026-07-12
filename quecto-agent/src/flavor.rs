use crate::BoxErr;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Lowercase hex SHA-256 of the given text.
pub fn content_hash(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A flavor manifest. Every scalar field is optional so manifests can be merged
/// key-by-key; omitted keys inherit from the layer below.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Flavor {
    pub name: Option<String>,
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub max_steps: Option<usize>,
    pub auto_verify: Option<bool>,
    pub auto_approve: Option<bool>,
    pub system_prompt: Option<String>,
    pub system_prompt_file: Option<String>,
    #[serde(default)]
    pub tools: ToolsSection,
    #[serde(default)]
    pub approval: ApprovalSection,
    #[serde(default)]
    pub verify: VerifySection,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolsSection {
    pub enabled: Option<Vec<String>>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ApprovalSection {
    pub preset: Option<String>,
    /// Per-operation overrides such as `run_command = "allow"`.
    #[serde(flatten)]
    pub overrides: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifySection {
    pub test: Option<String>,
    pub lint: Option<String>,
    pub build: Option<String>,
    pub required: Option<Vec<String>>,
}

fn or<T>(base: Option<T>, over: Option<T>) -> Option<T> {
    over.or(base)
}

impl Flavor {
    /// Parse one manifest.
    pub fn parse(text: &str) -> Result<Flavor, BoxErr> {
        Ok(toml::from_str(text)?)
    }

    /// Load a manifest from a file, if it exists. Missing file → `Ok(None)`.
    pub fn load(path: &Path) -> Result<Option<Flavor>, BoxErr> {
        match std::fs::read_to_string(path) {
            Ok(text) => Ok(Some(Flavor::parse(&text)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(Box::new(e)),
        }
    }

    /// Merge `over` on top of `self`: `over`'s set keys win; unset keys inherit.
    pub fn merge(self, over: Flavor) -> Flavor {
        let mut overrides = self.approval.overrides;
        overrides.extend(over.approval.overrides);
        Flavor {
            name: or(self.name, over.name),
            model: or(self.model, over.model),
            base_url: or(self.base_url, over.base_url),
            max_steps: or(self.max_steps, over.max_steps),
            auto_verify: or(self.auto_verify, over.auto_verify),
            auto_approve: or(self.auto_approve, over.auto_approve),
            system_prompt: or(self.system_prompt, over.system_prompt),
            system_prompt_file: or(self.system_prompt_file, over.system_prompt_file),
            tools: ToolsSection {
                enabled: or(self.tools.enabled, over.tools.enabled),
            },
            approval: ApprovalSection {
                preset: or(self.approval.preset, over.approval.preset),
                overrides,
            },
            verify: VerifySection {
                test: or(self.verify.test, over.verify.test),
                lint: or(self.verify.lint, over.verify.lint),
                build: or(self.verify.build, over.verify.build),
                required: or(self.verify.required, over.verify.required),
            },
        }
    }

    /// The `[verify]` commands in a stable order (test, lint, build), skipping
    /// unset ones.
    pub fn verify_commands(&self) -> Vec<String> {
        [&self.verify.test, &self.verify.lint, &self.verify.build]
            .into_iter()
            .flatten()
            .cloned()
            .collect()
    }

    /// True if this flavor declares shell verification commands or loosens
    /// approval — the fields a project flavor may apply only once trusted.
    pub fn wants_privilege(&self) -> bool {
        !self.verify_commands().is_empty() || approval_loosens(&self.approval)
    }

    /// Human-readable lines describing what a trust prompt would grant.
    pub fn privilege_summary(&self) -> Vec<String> {
        let mut lines = Vec::new();
        let commands = self.verify_commands();
        if !commands.is_empty() {
            lines.push(format!("run commands: {}", commands.join(", ")));
        }
        if approval_loosens(&self.approval) {
            let preset = self.approval.preset.as_deref().unwrap_or("(overrides)");
            lines.push(format!("loosen approval: preset {preset}"));
        }
        lines
    }
}

fn approval_loosens(approval: &ApprovalSection) -> bool {
    let preset_loosens = approval
        .preset
        .as_deref()
        .and_then(crate::policy::Preset::parse)
        .is_some_and(|p| !matches!(p, crate::policy::Preset::ReadOnly));
    let override_allows = approval
        .overrides
        .values()
        .any(|v| v.trim().eq_ignore_ascii_case("allow"));
    preset_loosens || override_allows
}

/// Where a flavor layer comes from. Project layers are not fully trusted (M7b
/// gates their command-bearing fields).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Scope {
    User,
    Project,
}

/// The ordered flavor files to merge, low → high precedence, tagged by scope.
/// `home` and `cwd` are injected for testability.
pub fn layer_paths(home: &Path, cwd: &Path, flavor_name: Option<&str>) -> Vec<(Scope, PathBuf)> {
    let user = home.join(".config").join("quecto");
    let project = cwd.join(".quecto");
    let mut paths = vec![(Scope::User, user.join("flavor.toml"))];
    if let Some(name) = flavor_name {
        paths.push((
            Scope::User,
            user.join("flavors").join(format!("{name}.toml")),
        ));
    }
    paths.push((Scope::Project, project.join("flavor.toml")));
    if let Some(name) = flavor_name {
        paths.push((
            Scope::Project,
            project.join("flavors").join(format!("{name}.toml")),
        ));
    }
    paths
}

/// Concatenate the raw text of existing project-scope layer files (in layer
/// order). Returns `None` when no project flavor file exists.
pub fn project_raw(home: &Path, cwd: &Path, flavor_name: Option<&str>) -> Option<String> {
    let mut parts = Vec::new();
    for (scope, path) in layer_paths(home, cwd, flavor_name) {
        if scope != Scope::Project {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(&path) {
            parts.push(text);
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n"))
    }
}

/// Merge every existing layer, low → high precedence, into one flavor.
pub fn resolve(home: &Path, cwd: &Path, flavor_name: Option<&str>) -> Result<Flavor, BoxErr> {
    let mut merged = Flavor::default();
    for (_scope, path) in layer_paths(home, cwd, flavor_name) {
        if let Some(layer) = Flavor::load(&path)? {
            merged = merged.merge(layer);
        }
    }
    Ok(merged)
}

/// Resolve user-scope and project-scope layers separately so a caller can apply
/// project command-bearing fields only when trusted (M7b).
pub fn resolve_scoped(
    home: &Path,
    cwd: &Path,
    flavor_name: Option<&str>,
) -> Result<(Flavor, Flavor), BoxErr> {
    let mut user = Flavor::default();
    let mut project = Flavor::default();
    for (scope, path) in layer_paths(home, cwd, flavor_name) {
        if let Some(layer) = Flavor::load(&path)? {
            match scope {
                Scope::User => user = user.merge(layer),
                Scope::Project => project = project.merge(layer),
            }
        }
    }
    Ok((user, project))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn parse_reads_known_fields_and_sections() {
        let f = Flavor::parse(
            r#"
name = "reviewer"
model = "m1"
max_steps = 30
auto_verify = true
system_prompt = "be terse"

[tools]
enabled = ["read_file", "search_text"]

[approval]
preset = "read-only"
run_command = "ask"

[verify]
test = "cargo test"
required = ["test"]
"#,
        )
        .unwrap();
        assert_eq!(f.name.as_deref(), Some("reviewer"));
        assert_eq!(f.model.as_deref(), Some("m1"));
        assert_eq!(f.max_steps, Some(30));
        assert_eq!(f.auto_verify, Some(true));
        assert_eq!(
            f.tools.enabled.as_deref().unwrap(),
            ["read_file", "search_text"]
        );
        assert_eq!(f.approval.preset.as_deref(), Some("read-only"));
        assert_eq!(
            f.approval.overrides.get("run_command").map(String::as_str),
            Some("ask")
        );
        assert_eq!(f.verify_commands(), vec!["cargo test".to_string()]);
    }

    #[test]
    fn merge_lets_higher_layer_win_and_inherits_unset() {
        let base = Flavor::parse(
            r#"model = "base-model"
max_steps = 10
system_prompt = "base""#,
        )
        .unwrap();
        let over = Flavor::parse(r#"model = "over-model""#).unwrap();
        let merged = base.merge(over);
        assert_eq!(merged.model.as_deref(), Some("over-model"));
        assert_eq!(merged.max_steps, Some(10));
        assert_eq!(merged.system_prompt.as_deref(), Some("base"));
    }

    #[test]
    fn merge_unions_approval_overrides() {
        let base = Flavor::parse("[approval]\nrun_command = \"ask\"").unwrap();
        let over = Flavor::parse("[approval]\nwrite_file = \"allow\"").unwrap();
        let merged = base.merge(over);
        assert_eq!(
            merged
                .approval
                .overrides
                .get("run_command")
                .map(String::as_str),
            Some("ask")
        );
        assert_eq!(
            merged
                .approval
                .overrides
                .get("write_file")
                .map(String::as_str),
            Some("allow")
        );
    }

    #[test]
    fn unknown_top_level_key_is_rejected() {
        assert!(Flavor::parse("bogus_key = 1").is_err());
    }

    #[test]
    fn layer_paths_are_ordered_user_then_project() {
        let paths = layer_paths(Path::new("/home/u"), Path::new("/repo"), Some("rev"));
        let shown: Vec<String> = paths.iter().map(|(_, p)| p.display().to_string()).collect();
        assert_eq!(
            shown,
            vec![
                "/home/u/.config/quecto/flavor.toml".to_string(),
                "/home/u/.config/quecto/flavors/rev.toml".to_string(),
                "/repo/.quecto/flavor.toml".to_string(),
                "/repo/.quecto/flavors/rev.toml".to_string(),
            ]
        );
        assert_eq!(paths[0].0, Scope::User);
        assert_eq!(paths[3].0, Scope::Project);
    }

    #[test]
    fn resolve_merges_existing_layers_low_to_high() {
        use std::fs;
        let home = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        fs::create_dir_all(home.path().join(".config/quecto")).unwrap();
        fs::create_dir_all(repo.path().join(".quecto")).unwrap();
        fs::write(
            home.path().join(".config/quecto/flavor.toml"),
            "model = \"user-model\"\nmax_steps = 5",
        )
        .unwrap();
        fs::write(
            repo.path().join(".quecto/flavor.toml"),
            "model = \"project-model\"",
        )
        .unwrap();
        let f = resolve(home.path(), repo.path(), None).unwrap();
        assert_eq!(f.model.as_deref(), Some("project-model"));
        assert_eq!(f.max_steps, Some(5));
    }

    #[test]
    fn resolve_returns_default_when_no_layers_exist() {
        let home = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        let f = resolve(home.path(), repo.path(), None).unwrap();
        assert!(f.model.is_none());
        assert!(f.tools.enabled.is_none());
    }

    #[test]
    fn resolve_scoped_separates_user_and_project() {
        use std::fs;
        let home = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        fs::create_dir_all(home.path().join(".config/quecto")).unwrap();
        fs::create_dir_all(repo.path().join(".quecto")).unwrap();
        fs::write(
            home.path().join(".config/quecto/flavor.toml"),
            "model = \"u\"",
        )
        .unwrap();
        fs::write(
            repo.path().join(".quecto/flavor.toml"),
            "[verify]\ntest = \"cargo test\"",
        )
        .unwrap();
        let (user, project) = resolve_scoped(home.path(), repo.path(), None).unwrap();
        assert_eq!(user.model.as_deref(), Some("u"));
        assert_eq!(project.verify.test.as_deref(), Some("cargo test"));
    }

    #[test]
    fn content_hash_is_stable_and_sensitive() {
        let a = content_hash("model = \"x\"");
        let b = content_hash("model = \"x\"");
        let c = content_hash("model = \"y\"");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|ch| ch.is_ascii_hexdigit()));
    }

    #[test]
    fn project_raw_concatenates_existing_project_files() {
        use std::fs;
        let home = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        fs::create_dir_all(repo.path().join(".quecto")).unwrap();
        fs::write(repo.path().join(".quecto/flavor.toml"), "model = \"p\"").unwrap();
        let raw = project_raw(home.path(), repo.path(), None).unwrap();
        assert!(raw.contains("model = \"p\""));
        // No project files → None.
        let empty = tempfile::tempdir().unwrap();
        assert!(project_raw(home.path(), empty.path(), None).is_none());
    }

    #[test]
    fn wants_privilege_true_for_verify_or_loosening() {
        let verify = Flavor::parse("[verify]\ntest = \"cargo test\"").unwrap();
        assert!(verify.wants_privilege());
        let full = Flavor::parse("[approval]\npreset = \"full\"").unwrap();
        assert!(full.wants_privilege());
        let loosen = Flavor::parse("[approval]\nrun_command = \"allow\"").unwrap();
        assert!(loosen.wants_privilege());
    }

    #[test]
    fn wants_privilege_false_for_safe_or_tightening_fields() {
        let safe = Flavor::parse(
            "model = \"m\"\nsystem_prompt = \"hi\"\n[tools]\nenabled=[\"read_file\"]",
        )
        .unwrap();
        assert!(!safe.wants_privilege());
        let readonly =
            Flavor::parse("[approval]\npreset = \"read-only\"\nrun_command = \"deny\"").unwrap();
        assert!(!readonly.wants_privilege());
    }

    #[test]
    fn privilege_summary_lists_commands_and_loosening() {
        let f = Flavor::parse(
            "[approval]\npreset=\"full\"\n[verify]\ntest=\"cargo test\"\nlint=\"cargo clippy\"",
        )
        .unwrap();
        let lines = f.privilege_summary().join("\n");
        assert!(lines.contains("cargo test"));
        assert!(lines.contains("cargo clippy"));
        assert!(lines.to_lowercase().contains("approval"));
    }
}
