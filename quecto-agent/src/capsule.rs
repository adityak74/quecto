use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Pull the contents of the first fenced code block (```` ``` ````...```` ``` ````,
/// optional language tag on the opening fence ignored) out of `text`. Used by
/// `/capsule-create` to extract the drafted `CAPSULE.md` body from the model's
/// reply.
pub fn extract_fenced_block(text: &str) -> Result<String, String> {
    const ERR: &str = "no fenced code block found in model output";
    let start = text.find("```").ok_or_else(|| ERR.to_string())?;
    let after_start = &text[start + 3..];
    let content_start = after_start.find('\n').map(|i| i + 1).unwrap_or(0);
    let rest = &after_start[content_start..];
    let end = rest.find("```").ok_or_else(|| ERR.to_string())?;
    Ok(rest[..end].trim_end().to_string())
}

/// One capsule: instructions (and optionally scripts) parsed from a `CAPSULE.md`.
#[derive(Clone, Debug, PartialEq)]
pub struct Capsule {
    pub name: String,
    pub description: String,
    pub instructions: String,
    pub dir: PathBuf,
    /// Session state keys this capsule declares it writes, from the optional
    /// `writes:` frontmatter field (comma-separated). Empty for capsules that
    /// only inject instructions, which is every capsule today — no capsule
    /// capability writes shared session state yet. Declared purely so two
    /// capsules that claim the same key can be caught at `/load` time before
    /// either one runs, rather than surfacing as unexplained cross-capsule
    /// behavior later.
    pub writes: Vec<String>,
}

impl Capsule {
    /// Parse a `CAPSULE.md` file's contents into a `Capsule`. Public so
    /// `/capsule-create` can validate a model-drafted capsule body before
    /// writing it to disk, reusing the exact same validation discovery uses.
    pub fn parse(text: &str, dir: PathBuf) -> Result<Capsule, String> {
        let (frontmatter, body) = split_frontmatter(text)?;
        let fields = parse_frontmatter_fields(&frontmatter)?;
        let name = fields
            .get("name")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "missing 'name' in frontmatter".to_string())?;
        let description = fields.get("description").cloned().unwrap_or_default();
        let writes = fields
            .get("writes")
            .map(|raw| {
                raw.split(',')
                    .map(|key| key.trim().to_string())
                    .filter(|key| !key.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        Ok(Capsule {
            name,
            description,
            instructions: body.trim().to_string(),
            dir,
            writes,
        })
    }

    /// This capsule's `scripts/` directory, if it exists.
    pub fn scripts_dir(&self) -> Option<PathBuf> {
        let dir = self.dir.join("scripts");
        dir.is_dir().then_some(dir)
    }

    /// The block folded into the system prompt while this capsule is active.
    pub fn system_prompt_section(&self) -> String {
        let mut section = format!("## Capsule: {}\n{}", self.name, self.instructions);
        if let Some(scripts) = self.scripts_dir() {
            section.push_str(&format!(
                "\n\nScripts for this capsule are available at: {}",
                scripts.display()
            ));
        }
        section
    }
}

/// Split `---\n<frontmatter>\n---\n<body>` into `(frontmatter, body)`.
fn split_frontmatter(text: &str) -> Result<(String, String), String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let rest = text
        .strip_prefix("---")
        .ok_or_else(|| "missing frontmatter delimiter".to_string())?;
    let rest = rest.strip_prefix('\n').unwrap_or(rest);
    let end = rest
        .find("\n---")
        .ok_or_else(|| "missing closing frontmatter delimiter".to_string())?;
    let frontmatter = rest[..end].to_string();
    let after = &rest[end + "\n---".len()..];
    let body = after.strip_prefix('\n').unwrap_or(after);
    Ok((frontmatter, body.to_string()))
}

/// Parse flat `key: value` lines. Not a general YAML parser — capsules only
/// use flat scalar frontmatter fields (`name`, `description`).
fn parse_frontmatter_fields(text: &str) -> Result<BTreeMap<String, String>, String> {
    let mut fields = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let (key, value) = line
            .split_once(':')
            .ok_or_else(|| format!("malformed frontmatter line: {line}"))?;
        let value = value.trim().trim_matches('"').trim_matches('\'');
        fields.insert(key.trim().to_string(), value.to_string());
    }
    Ok(fields)
}

/// Command names a capsule can never shadow — built-ins always win.
pub const RESERVED_NAMES: &[&str] = &[
    "help", "h", "?", "model", "context", "diff", "status", "undo", "approve", "deny", "clear",
    "exit", "quit", "q", "reasoning", "tools", "commands", "capsules", "load", "unload",
    "capsule-create",
];

/// Whether `name` collides with a reserved built-in command (case-insensitive).
pub fn is_reserved(name: &str) -> bool {
    RESERVED_NAMES.iter().any(|r| r.eq_ignore_ascii_case(name))
}

/// The set of discovered capsules, merged from user and project scope with
/// project taking precedence over user for a shared name.
#[derive(Clone, Debug, Default)]
pub struct CapsuleRegistry {
    capsules: BTreeMap<String, Capsule>,
}

impl CapsuleRegistry {
    /// Scan `user_dir` then `project_dir` for `<name>/CAPSULE.md` capsules,
    /// merging by name with `project_dir` overriding `user_dir`. Missing
    /// directories are treated as empty. Malformed capsules, reserved-name
    /// collisions, and duplicate names within one scope are skipped with a
    /// warning on stderr. Name matching is case-insensitive.
    pub fn discover(user_dir: &Path, project_dir: &Path) -> CapsuleRegistry {
        let mut capsules = scan_scope(user_dir);
        for (project_key, capsule) in scan_scope(project_dir) {
            // Normalize the key for case-insensitive merge, but preserve original case
            let normalized_key = project_key.to_lowercase();
            capsules.insert(normalized_key, capsule);
        }
        CapsuleRegistry { capsules }
    }

    /// Look up a capsule by name, case-insensitively.
    pub fn get(&self, name: &str) -> Option<&Capsule> {
        self.capsules.values().find(|c| c.name.eq_ignore_ascii_case(name))
    }

    /// All discovered capsule names, in each capsule's originally-declared
    /// case. Not sorted — iteration order follows the internal map's
    /// lowercased keys, not the declared-case names; callers that need a
    /// sorted display (e.g. `/capsules`) sort separately (see `list_display`).
    pub fn names(&self) -> Vec<String> {
        self.capsules.values().map(|c| c.name.clone()).collect()
    }

    /// All discovered capsules.
    pub fn iter(&self) -> impl Iterator<Item = &Capsule> {
        self.capsules.values()
    }

    /// Insert or replace a capsule in the registry (case-insensitive key).
    /// Used by `/capsule-create` to register a freshly drafted capsule
    /// without a full re-scan of disk.
    pub fn insert(&mut self, capsule: Capsule) {
        let key = capsule.name.to_lowercase();
        self.capsules.insert(key, capsule);
    }
}

/// Scan one scope directory for capsule subdirectories, deduping by name
/// (first by directory scan order wins, with a warning for later duplicates).
/// Names are matched case-insensitively, but the Capsule's original-case name is preserved.
fn scan_scope(dir: &Path) -> BTreeMap<String, Capsule> {
    let mut found: BTreeMap<String, Capsule> = BTreeMap::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return found;
    };
    let mut entries: Vec<_> = entries.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let capsule_file = path.join("CAPSULE.md");
        let text = match fs::read_to_string(&capsule_file) {
            Ok(t) => t,
            Err(_) => continue,
        };
        match Capsule::parse(&text, path.clone()) {
            Ok(capsule) => {
                if is_reserved(&capsule.name) {
                    eprintln!(
                        "quecto-agent: capsule \"{}\" at {} shadows a built-in command, skipping",
                        capsule.name,
                        path.display()
                    );
                    continue;
                }
                let normalized_key = capsule.name.to_lowercase();
                if let Some(existing) = found.get(&normalized_key) {
                    eprintln!(
                        "quecto-agent: capsule \"{}\" at {} shadowed by {} (duplicate name in scope), skipping",
                        capsule.name,
                        path.display(),
                        existing.dir.display()
                    );
                    continue;
                }
                found.insert(normalized_key, capsule);
            }
            Err(reason) => {
                eprintln!(
                    "quecto-agent: skipping capsule at {}: {reason}",
                    capsule_file.display()
                );
            }
        }
    }
    found
}

/// The active (loaded) capsule set for one REPL session, plus the registry of
/// everything discoverable.
pub struct CapsuleState {
    registry: CapsuleRegistry,
    base_system_prompt: String,
    active: Vec<String>,
}

impl CapsuleState {
    pub fn new(registry: CapsuleRegistry, base_system_prompt: String) -> CapsuleState {
        CapsuleState {
            registry,
            base_system_prompt,
            active: Vec::new(),
        }
    }

    pub fn registry(&self) -> &CapsuleRegistry {
        &self.registry
    }

    pub fn is_active(&self, name: &str) -> bool {
        self.active.iter().any(|n| n.eq_ignore_ascii_case(name))
    }

    /// Load a capsule by name. `Ok(true)` if newly loaded, `Ok(false)` if it
    /// was already active, `Err` with a user-facing message if unknown or if
    /// its declared `writes` surface conflicts with an already-active
    /// capsule's.
    pub fn load(&mut self, name: &str) -> Result<bool, String> {
        let Some(capsule) = self.registry.get(name) else {
            return Err(format!("no such capsule: {name} (see /capsules)"));
        };
        if self.is_active(&capsule.name) {
            return Ok(false);
        }
        if let Some((key, other)) = self.write_conflict(capsule) {
            return Err(format!(
                "cannot load {}: write key \"{key}\" is already claimed by active capsule {other}",
                capsule.name
            ));
        }
        self.active.push(capsule.name.clone());
        Ok(true)
    }

    /// The first session-state key `candidate` declares writing that some
    /// already-active capsule also declares writing, plus that capsule's
    /// name. `None` if there's no overlap.
    fn write_conflict(&self, candidate: &Capsule) -> Option<(String, String)> {
        for active_name in &self.active {
            let Some(active_capsule) = self.registry.get(active_name) else {
                continue;
            };
            for key in &candidate.writes {
                if active_capsule.writes.contains(key) {
                    return Some((key.clone(), active_capsule.name.clone()));
                }
            }
        }
        None
    }

    /// Unload a capsule by name. Returns whether it had been active.
    pub fn unload(&mut self, name: &str) -> bool {
        let before = self.active.len();
        self.active.retain(|n| !n.eq_ignore_ascii_case(name));
        self.active.len() != before
    }

    /// Register `capsule` in the live registry and immediately mark it
    /// active. Used by `/capsule-create` right after writing a freshly
    /// drafted `CAPSULE.md` to disk — the capsule is usable on the very
    /// next turn without a separate `/load`.
    pub fn create_and_load(&mut self, capsule: Capsule) {
        let name = capsule.name.clone();
        self.registry.insert(capsule);
        if !self.is_active(&name) {
            self.active.push(name);
        }
    }

    /// The system prompt with every active capsule's instructions folded in,
    /// in load order.
    pub fn render_system_prompt(&self) -> String {
        let mut prompt = self.base_system_prompt.clone();
        for name in &self.active {
            if let Some(capsule) = self.registry.get(name) {
                prompt.push_str("\n\n");
                prompt.push_str(&capsule.system_prompt_section());
            }
        }
        prompt
    }

    /// One line per discovered capsule, sorted by name, marking active ones
    /// with `●`. Used by `/capsules`.
    pub fn list_display(&self) -> String {
        let mut capsules: Vec<&Capsule> = self.registry.iter().collect();
        capsules.sort_by(|a, b| a.name.cmp(&b.name));
        if capsules.is_empty() {
            return "no capsules found".to_string();
        }
        capsules
            .iter()
            .map(|c| {
                let marker = if self.is_active(&c.name) { "●" } else { " " };
                if c.description.is_empty() {
                    format!("{marker} {}", c.name)
                } else {
                    format!("{marker} {} — {}", c.name, c.description)
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// `~/.quecto/capsules`, the user (personal) capsule scope. `None` if `HOME`
/// is not set.
pub fn default_user_capsules_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".quecto").join("capsules"))
}

/// `<cwd>/.quecto/capsules`, the project capsule scope.
pub fn project_capsules_dir(cwd: &Path) -> PathBuf {
    cwd.join(".quecto").join("capsules")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn parses_name_description_and_body() {
        let capsule = Capsule::parse(
            "---\nname: demo\ndescription: demo capsule\n---\nFollow the demo workflow.",
            PathBuf::from("/capsules/demo"),
        )
        .unwrap();
        assert_eq!(capsule.name, "demo");
        assert_eq!(capsule.description, "demo capsule");
        assert_eq!(capsule.instructions, "Follow the demo workflow.");
        assert_eq!(capsule.dir, PathBuf::from("/capsules/demo"));
    }

    #[test]
    fn description_defaults_to_empty_when_missing() {
        let capsule = Capsule::parse("---\nname: demo\n---\nbody", PathBuf::from(".")).unwrap();
        assert_eq!(capsule.description, "");
    }

    #[test]
    fn writes_defaults_to_empty_when_missing() {
        let capsule = Capsule::parse("---\nname: demo\n---\nbody", PathBuf::from(".")).unwrap();
        assert!(capsule.writes.is_empty());
    }

    #[test]
    fn writes_parses_comma_separated_keys() {
        let capsule = Capsule::parse(
            "---\nname: demo\nwrites: theme, active_branch\n---\nbody",
            PathBuf::from("."),
        )
        .unwrap();
        assert_eq!(capsule.writes, vec!["theme".to_string(), "active_branch".to_string()]);
    }

    #[test]
    fn errors_when_name_missing() {
        let err = Capsule::parse("---\ndescription: no name\n---\nbody", PathBuf::from(".")).unwrap_err();
        assert!(err.contains("name"));
    }

    #[test]
    fn errors_when_opening_delimiter_missing() {
        let err = Capsule::parse("name: demo\n---\nbody", PathBuf::from(".")).unwrap_err();
        assert!(err.contains("frontmatter delimiter"));
    }

    #[test]
    fn errors_when_closing_delimiter_missing() {
        let err = Capsule::parse("---\nname: demo\nbody with no closer", PathBuf::from(".")).unwrap_err();
        assert!(err.contains("closing frontmatter delimiter"));
    }

    #[test]
    fn scripts_dir_is_none_when_absent() {
        let dir = tempdir().unwrap();
        let capsule = Capsule {
            name: "demo".to_string(),
            description: String::new(),
            instructions: String::new(),
            dir: dir.path().to_path_buf(),
            writes: Vec::new(),
        };
        assert_eq!(capsule.scripts_dir(), None);
    }

    #[test]
    fn scripts_dir_is_some_when_present() {
        let dir = tempdir().unwrap();
        fs::create_dir(dir.path().join("scripts")).unwrap();
        let capsule = Capsule {
            name: "demo".to_string(),
            description: String::new(),
            instructions: String::new(),
            dir: dir.path().to_path_buf(),
            writes: Vec::new(),
        };
        assert_eq!(capsule.scripts_dir(), Some(dir.path().join("scripts")));
    }

    #[test]
    fn system_prompt_section_includes_name_and_instructions() {
        let capsule = Capsule {
            name: "demo".to_string(),
            description: String::new(),
            instructions: "Follow the demo workflow.".to_string(),
            dir: PathBuf::from("/nonexistent"),
            writes: Vec::new(),
        };
        let section = capsule.system_prompt_section();
        assert!(section.contains("## Capsule: demo"));
        assert!(section.contains("Follow the demo workflow."));
    }

    #[test]
    fn system_prompt_section_mentions_scripts_path_when_present() {
        let dir = tempdir().unwrap();
        fs::create_dir(dir.path().join("scripts")).unwrap();
        let capsule = Capsule {
            name: "demo".to_string(),
            description: String::new(),
            instructions: "body".to_string(),
            dir: dir.path().to_path_buf(),
            writes: Vec::new(),
        };
        let section = capsule.system_prompt_section();
        assert!(section.contains(&dir.path().join("scripts").display().to_string()));
    }

    fn write_capsule(root: &Path, name: &str, description: &str, body: &str) {
        let dir = root.join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("CAPSULE.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\n{body}"),
        )
        .unwrap();
    }

    #[test]
    fn missing_scope_directories_yield_an_empty_registry() {
        let registry = CapsuleRegistry::discover(
            Path::new("/does/not/exist/user"),
            Path::new("/does/not/exist/project"),
        );
        assert!(registry.names().is_empty());
    }

    #[test]
    fn discovers_capsules_from_both_scopes() {
        let user = tempdir().unwrap();
        let project = tempdir().unwrap();
        write_capsule(user.path(), "alpha", "from user", "alpha body");
        write_capsule(project.path(), "beta", "from project", "beta body");

        let registry = CapsuleRegistry::discover(user.path(), project.path());

        assert_eq!(registry.get("alpha").unwrap().description, "from user");
        assert_eq!(registry.get("beta").unwrap().description, "from project");
    }

    #[test]
    fn project_scope_overrides_user_scope_for_same_name() {
        let user = tempdir().unwrap();
        let project = tempdir().unwrap();
        write_capsule(user.path(), "demo", "user version", "user body");
        write_capsule(project.path(), "demo", "project version", "project body");

        let registry = CapsuleRegistry::discover(user.path(), project.path());

        assert_eq!(registry.get("demo").unwrap().description, "project version");
        assert_eq!(registry.names(), vec!["demo".to_string()]);
    }

    #[test]
    fn reserved_name_is_skipped() {
        let project = tempdir().unwrap();
        write_capsule(project.path(), "help", "shadows a builtin", "body");

        let registry = CapsuleRegistry::discover(Path::new("/does/not/exist"), project.path());

        assert!(registry.get("help").is_none());
    }

    #[test]
    fn duplicate_name_within_one_scope_keeps_first_by_scan_order() {
        let project = tempdir().unwrap();
        write_capsule(project.path(), "aaa-demo", "first", "body");
        write_capsule(project.path(), "zzz-demo", "second", "body");
        // Both directories declare the same capsule name via frontmatter, but
        // "aaa-demo" sorts first, so it should win.
        fs::write(
            project.path().join("aaa-demo").join("CAPSULE.md"),
            "---\nname: demo\ndescription: first\n---\nbody",
        )
        .unwrap();
        fs::write(
            project.path().join("zzz-demo").join("CAPSULE.md"),
            "---\nname: demo\ndescription: second\n---\nbody",
        )
        .unwrap();

        let registry = CapsuleRegistry::discover(Path::new("/does/not/exist"), project.path());

        assert_eq!(registry.get("demo").unwrap().description, "first");
    }

    #[test]
    fn malformed_capsule_is_skipped_but_siblings_still_load() {
        let project = tempdir().unwrap();
        write_capsule(project.path(), "good", "fine", "body");
        fs::create_dir_all(project.path().join("bad")).unwrap();
        fs::write(project.path().join("bad").join("CAPSULE.md"), "not frontmatter at all").unwrap();

        let registry = CapsuleRegistry::discover(Path::new("/does/not/exist"), project.path());

        assert!(registry.get("good").is_some());
        assert!(registry.get("bad").is_none());
    }

    #[test]
    fn get_matches_case_insensitively() {
        let project = tempdir().unwrap();
        write_capsule(project.path(), "Demo", "x", "body");
        let registry = CapsuleRegistry::discover(Path::new("/does/not/exist"), project.path());
        assert_eq!(registry.get("demo").unwrap().name, "Demo");
    }

    #[test]
    fn project_scope_overrides_user_scope_even_with_different_case() {
        let user = tempdir().unwrap();
        let project = tempdir().unwrap();
        write_capsule(user.path(), "Demo", "user version", "user body");
        write_capsule(project.path(), "demo", "project version", "project body");

        let registry = CapsuleRegistry::discover(user.path(), project.path());

        assert_eq!(registry.names().len(), 1);
        assert_eq!(registry.get("demo").unwrap().description, "project version");
    }

    #[test]
    fn duplicate_name_within_one_scope_is_case_insensitive() {
        let project = tempdir().unwrap();
        write_capsule(project.path(), "aaa-demo", "first", "body");
        write_capsule(project.path(), "zzz-demo", "second", "body");
        fs::write(
            project.path().join("aaa-demo").join("CAPSULE.md"),
            "---\nname: Demo\ndescription: first\n---\nbody",
        )
        .unwrap();
        fs::write(
            project.path().join("zzz-demo").join("CAPSULE.md"),
            "---\nname: demo\ndescription: second\n---\nbody",
        )
        .unwrap();

        let registry = CapsuleRegistry::discover(Path::new("/does/not/exist"), project.path());

        assert_eq!(registry.names().len(), 1);
        assert_eq!(registry.get("demo").unwrap().description, "first");
    }

    fn registry_with(root: &Path, name: &str, description: &str, body: &str) -> CapsuleRegistry {
        write_capsule(root, name, description, body);
        CapsuleRegistry::discover(Path::new("/does/not/exist"), root)
    }

    #[test]
    fn load_unknown_capsule_errors() {
        let mut state = CapsuleState::new(CapsuleRegistry::default(), "base".to_string());
        let err = state.load("demo").unwrap_err();
        assert_eq!(err, "no such capsule: demo (see /capsules)");
    }

    #[test]
    fn load_marks_active_then_is_idempotent() {
        let dir = tempdir().unwrap();
        let registry = registry_with(dir.path(), "demo", "d", "body");
        let mut state = CapsuleState::new(registry, "base".to_string());

        assert_eq!(state.load("demo"), Ok(true));
        assert!(state.is_active("demo"));
        assert_eq!(state.load("demo"), Ok(false));
    }

    #[test]
    fn load_rejects_a_capsule_whose_write_key_is_already_claimed() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("alpha")).unwrap();
        fs::write(
            dir.path().join("alpha").join("CAPSULE.md"),
            "---\nname: alpha\nwrites: theme\n---\nbody",
        )
        .unwrap();
        fs::create_dir_all(dir.path().join("beta")).unwrap();
        fs::write(
            dir.path().join("beta").join("CAPSULE.md"),
            "---\nname: beta\nwrites: theme\n---\nbody",
        )
        .unwrap();
        let registry = CapsuleRegistry::discover(Path::new("/does/not/exist"), dir.path());
        let mut state = CapsuleState::new(registry, "base".to_string());

        assert_eq!(state.load("alpha"), Ok(true));
        let err = state.load("beta").unwrap_err();
        assert!(err.contains("theme"));
        assert!(err.contains("alpha"));
        assert!(!state.is_active("beta"));
    }

    #[test]
    fn load_allows_capsules_with_disjoint_write_keys() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("alpha")).unwrap();
        fs::write(
            dir.path().join("alpha").join("CAPSULE.md"),
            "---\nname: alpha\nwrites: theme\n---\nbody",
        )
        .unwrap();
        fs::create_dir_all(dir.path().join("beta")).unwrap();
        fs::write(
            dir.path().join("beta").join("CAPSULE.md"),
            "---\nname: beta\nwrites: layout\n---\nbody",
        )
        .unwrap();
        let registry = CapsuleRegistry::discover(Path::new("/does/not/exist"), dir.path());
        let mut state = CapsuleState::new(registry, "base".to_string());

        assert_eq!(state.load("alpha"), Ok(true));
        assert_eq!(state.load("beta"), Ok(true));
        assert!(state.is_active("alpha"));
        assert!(state.is_active("beta"));
    }

    #[test]
    fn unload_reports_whether_it_was_active() {
        let dir = tempdir().unwrap();
        let registry = registry_with(dir.path(), "demo", "d", "body");
        let mut state = CapsuleState::new(registry, "base".to_string());

        assert!(!state.unload("demo"));
        state.load("demo").unwrap();
        assert!(state.unload("demo"));
        assert!(!state.is_active("demo"));
    }

    #[test]
    fn render_system_prompt_appends_active_capsules_in_load_order() {
        let dir = tempdir().unwrap();
        write_capsule(dir.path(), "first", "d1", "first body");
        write_capsule(dir.path(), "second", "d2", "second body");
        let registry = CapsuleRegistry::discover(Path::new("/does/not/exist"), dir.path());
        let mut state = CapsuleState::new(registry, "base prompt".to_string());

        state.load("second").unwrap();
        state.load("first").unwrap();
        let prompt = state.render_system_prompt();

        assert!(prompt.starts_with("base prompt"));
        let second_at = prompt.find("## Capsule: second").unwrap();
        let first_at = prompt.find("## Capsule: first").unwrap();
        assert!(second_at < first_at, "capsules must appear in load order");
    }

    #[test]
    fn render_system_prompt_excludes_unloaded_capsules() {
        let dir = tempdir().unwrap();
        let registry = registry_with(dir.path(), "demo", "d", "demo body");
        let mut state = CapsuleState::new(registry, "base".to_string());

        state.load("demo").unwrap();
        state.unload("demo");

        assert_eq!(state.render_system_prompt(), "base");
    }

    #[test]
    fn list_display_reports_when_registry_is_empty() {
        let state = CapsuleState::new(CapsuleRegistry::default(), "base".to_string());
        assert_eq!(state.list_display(), "no capsules found");
    }

    #[test]
    fn list_display_marks_active_capsules_and_sorts_by_name() {
        let dir = tempdir().unwrap();
        write_capsule(dir.path(), "zeta", "last", "body");
        write_capsule(dir.path(), "alpha", "first", "body");
        let registry = CapsuleRegistry::discover(Path::new("/does/not/exist"), dir.path());
        let mut state = CapsuleState::new(registry, "base".to_string());
        state.load("zeta").unwrap();

        let display = state.list_display();
        let lines: Vec<&str> = display.lines().collect();
        assert_eq!(lines, vec!["  alpha — first", "● zeta — last"]);
    }

    #[test]
    fn extract_fenced_block_returns_plain_fence_contents() {
        let text = "here you go:\n```\n---\nname: demo\n---\nbody\n```\nhope that helps";
        let block = extract_fenced_block(text).unwrap();
        assert_eq!(block, "---\nname: demo\n---\nbody");
    }

    #[test]
    fn extract_fenced_block_strips_language_tag() {
        let text = "```markdown\n---\nname: demo\n---\nbody\n```";
        let block = extract_fenced_block(text).unwrap();
        assert_eq!(block, "---\nname: demo\n---\nbody");
    }

    #[test]
    fn extract_fenced_block_uses_first_fence_when_multiple_present() {
        let text = "```\nfirst\n```\nand another:\n```\nsecond\n```";
        let block = extract_fenced_block(text).unwrap();
        assert_eq!(block, "first");
    }

    #[test]
    fn extract_fenced_block_errors_when_no_fence_present() {
        let err = extract_fenced_block("just plain text, no fences here").unwrap_err();
        assert_eq!(err, "no fenced code block found in model output");
    }

    #[test]
    fn extract_fenced_block_errors_when_fence_never_closes() {
        let err = extract_fenced_block("```\nname: demo\nno closing fence").unwrap_err();
        assert_eq!(err, "no fenced code block found in model output");
    }

    #[test]
    fn registry_insert_adds_a_capsule_findable_by_name() {
        let mut registry = CapsuleRegistry::default();
        registry.insert(Capsule {
            name: "demo".to_string(),
            description: "d".to_string(),
            instructions: "body".to_string(),
            dir: PathBuf::from("/capsules/demo"),
            writes: Vec::new(),
        });
        assert_eq!(registry.get("demo").unwrap().description, "d");
        assert_eq!(registry.names(), vec!["demo".to_string()]);
    }

    #[test]
    fn registry_insert_overwrites_existing_entry_of_same_name() {
        let mut registry = CapsuleRegistry::default();
        registry.insert(Capsule {
            name: "demo".to_string(),
            description: "old".to_string(),
            instructions: "body".to_string(),
            dir: PathBuf::from("/capsules/demo"),
            writes: Vec::new(),
        });
        registry.insert(Capsule {
            name: "demo".to_string(),
            description: "new".to_string(),
            instructions: "body".to_string(),
            dir: PathBuf::from("/capsules/demo"),
            writes: Vec::new(),
        });
        assert_eq!(registry.get("demo").unwrap().description, "new");
        assert_eq!(registry.names().len(), 1);
    }

    #[test]
    fn create_and_load_registers_and_activates_the_capsule() {
        let mut state = CapsuleState::new(CapsuleRegistry::default(), "base".to_string());
        state.create_and_load(Capsule {
            name: "demo".to_string(),
            description: "d".to_string(),
            instructions: "Follow the demo workflow.".to_string(),
            dir: PathBuf::from("/capsules/demo"),
            writes: Vec::new(),
        });

        assert!(state.is_active("demo"));
        assert!(state.registry().get("demo").is_some());
        assert!(state
            .render_system_prompt()
            .contains("Follow the demo workflow."));
    }

    #[test]
    fn create_and_load_is_idempotent_for_the_active_set() {
        let mut state = CapsuleState::new(CapsuleRegistry::default(), "base".to_string());
        let capsule = Capsule {
            name: "demo".to_string(),
            description: "d".to_string(),
            instructions: "Follow the demo workflow.".to_string(),
            dir: PathBuf::from("/capsules/demo"),
            writes: Vec::new(),
        };
        state.create_and_load(capsule.clone());
        state.create_and_load(capsule);

        // demo's instructions must appear exactly once in the rendered prompt,
        // not duplicated by a second entry in the active set.
        let prompt = state.render_system_prompt();
        assert_eq!(prompt.matches("Follow the demo workflow.").count(), 1);
    }

    #[test]
    fn capsule_create_is_a_reserved_name() {
        assert!(is_reserved("capsule-create"));
        assert!(is_reserved("Capsule-Create"));
    }

    #[test]
    fn project_capsules_dir_joins_dot_quecto_capsules() {
        assert_eq!(
            project_capsules_dir(Path::new("/repo")),
            PathBuf::from("/repo/.quecto/capsules")
        );
    }
}
