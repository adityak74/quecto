use std::collections::BTreeMap;
use std::path::PathBuf;

/// One capsule: instructions (and optionally scripts) parsed from a `CAPSULE.md`.
#[derive(Clone, Debug, PartialEq)]
pub struct Capsule {
    pub name: String,
    pub description: String,
    pub instructions: String,
    pub dir: PathBuf,
}

impl Capsule {
    /// Parse a `CAPSULE.md` file's contents. `dir` is the capsule's directory
    /// (the parent of the `CAPSULE.md` file), used for `scripts_dir`.
    #[allow(dead_code)]
    fn parse(text: &str, dir: PathBuf) -> Result<Capsule, String> {
        let (frontmatter, body) = split_frontmatter(text)?;
        let fields = parse_frontmatter_fields(&frontmatter)?;
        let name = fields
            .get("name")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "missing 'name' in frontmatter".to_string())?;
        let description = fields.get("description").cloned().unwrap_or_default();
        Ok(Capsule {
            name,
            description,
            instructions: body.trim().to_string(),
            dir,
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
#[allow(dead_code)]
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
#[allow(dead_code)]
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
        };
        let section = capsule.system_prompt_section();
        assert!(section.contains(&dir.path().join("scripts").display().to_string()));
    }
}
