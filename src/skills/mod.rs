/// A skill loaded from a `.md` file.
#[derive(Clone)]
pub struct Skill {
    /// Slash command name, e.g. `"/review"`.
    pub name: String,
    /// One-line description used in autocomplete.
    pub description: String,
    /// Prompt template. `{dir}` and `{args}` are substituted at invocation time.
    template: String,
}

impl Skill {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        template: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            template: template.into(),
        }
    }

    fn expand(&self, dir: &str, args: &str) -> String {
        self.template.replace("{dir}", dir).replace("{args}", args)
    }
}

/// Load skills from the global dir (`~/.kapitoshka/skills/`) and the
/// project-local dir (`<project_dir>/skills/`). Project-local skills with the
/// same name override global ones.
///
/// Each `.md` file contributes one skill. The file name (without extension)
/// becomes the slash command (e.g. `review.md` → `/review`).
///
/// Optional TOML front-matter delimited by `---` lines may contain a
/// `description` key. Everything after the front-matter is the prompt template.
pub fn load(project_dir: &str) -> Vec<Skill> {
    let mut skills: std::collections::HashMap<String, Skill> = std::collections::HashMap::new();

    // Load global skills first so project-local ones can override them.
    if let Some(home) = dirs_for_skills() {
        load_dir(&home, &mut skills);
    }
    load_dir(
        &std::path::PathBuf::from(project_dir).join("skills"),
        &mut skills,
    );

    let mut list: Vec<Skill> = skills.into_values().collect();
    list.sort_by(|a, b| a.name.cmp(&b.name));
    list
}

/// If `input` starts with a known skill command, expand and return the prompt.
/// Returns `None` for regular messages or built-in commands (`/model`).
pub fn resolve(skills: &[Skill], input: &str, dir: &str) -> Option<String> {
    let (cmd, args) = split_command(input);
    skills
        .iter()
        .find(|s| s.name == cmd)
        .map(|s| s.expand(dir, args))
}

// ── internals ──────────────────────────────────────────────────────────────────

fn dirs_for_skills() -> Option<std::path::PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(
        std::path::PathBuf::from(home)
            .join(".kapitoshka")
            .join("skills"),
    )
}

fn load_dir(dir: &std::path::Path, out: &mut std::collections::HashMap<String, Skill>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let stem = match path.file_stem().and_then(|s| s.to_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "failed to read skill file");
                continue;
            }
        };
        let (description, template) = parse_skill_file(&content, &stem);
        let name = format!("/{stem}");
        tracing::debug!(skill = %name, path = %path.display(), "loaded skill");
        out.insert(
            name.clone(),
            Skill {
                name,
                description,
                template,
            },
        );
    }
}

/// Parse optional TOML front-matter (`---` … `---`) and return
/// `(description, template_body)`.
fn parse_skill_file(content: &str, stem: &str) -> (String, String) {
    let content = content.trim_start();
    if !content.starts_with("---") {
        return (stem.to_string(), content.to_string());
    }
    // Find the closing `---`.
    let after_open = content.trim_start_matches('-').trim_start_matches('\n');
    let close = after_open.find("\n---");
    match close {
        None => (stem.to_string(), content.to_string()),
        Some(pos) => {
            let fm = &after_open[..pos];
            let body = after_open[pos + 4..].trim_start_matches('\n').to_string();
            let description = parse_description_from_toml(fm).unwrap_or_else(|| stem.to_string());
            (description, body)
        }
    }
}

fn parse_description_from_toml(fm: &str) -> Option<String> {
    for line in fm.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("description") {
            let rest = rest.trim();
            if let Some(rest) = rest.strip_prefix('=') {
                let val = rest.trim().trim_matches('"').trim_matches('\'');
                return Some(val.to_string());
            }
        }
    }
    None
}

fn split_command(input: &str) -> (&str, &str) {
    match input.find(char::is_whitespace) {
        Some(pos) => (&input[..pos], input[pos + 1..].trim()),
        None => (input, ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── split_command ─────────────────────────────────────────────────────────

    #[test]
    fn split_no_args() {
        assert_eq!(split_command("/review"), ("/review", ""));
    }

    #[test]
    fn split_with_args() {
        let (cmd, args) = split_command("/explain src/main.rs");
        assert_eq!(cmd, "/explain");
        assert_eq!(args, "src/main.rs");
    }

    #[test]
    fn split_trims_extra_whitespace() {
        let (cmd, args) = split_command("/explain   src/main.rs");
        assert_eq!(cmd, "/explain");
        assert_eq!(args, "src/main.rs");
    }

    // ── parse_skill_file ──────────────────────────────────────────────────────

    #[test]
    fn parse_no_frontmatter() {
        let (desc, body) = parse_skill_file("Do the thing in {dir}.", "review");
        assert_eq!(desc, "review");
        assert_eq!(body, "Do the thing in {dir}.");
    }

    #[test]
    fn parse_frontmatter_description() {
        let content = "---\ndescription = \"review the diff\"\n---\nRun git diff in {dir}.";
        let (desc, body) = parse_skill_file(content, "review");
        assert_eq!(desc, "review the diff");
        assert_eq!(body, "Run git diff in {dir}.");
    }

    #[test]
    fn parse_frontmatter_single_quotes() {
        let content = "---\ndescription = 'run tests'\n---\nVerify {dir}.";
        let (desc, body) = parse_skill_file(content, "verify");
        assert_eq!(desc, "run tests");
        assert_eq!(body, "Verify {dir}.");
    }

    #[test]
    fn parse_frontmatter_missing_description_falls_back_to_stem() {
        let content = "---\nauthor = \"alice\"\n---\nDo something.";
        let (desc, _) = parse_skill_file(content, "mytool");
        assert_eq!(desc, "mytool");
    }

    #[test]
    fn parse_unclosed_frontmatter_treated_as_body() {
        let content = "---\ndescription = \"oops\"\nno closing fence";
        let (desc, _) = parse_skill_file(content, "broken");
        assert_eq!(desc, "broken");
    }

    // ── Skill::expand ─────────────────────────────────────────────────────────

    #[test]
    fn expand_substitutes_dir_and_args() {
        let skill = Skill::new("/explain", "explain", "Explain {args} in {dir}.");
        assert_eq!(
            skill.expand("/home/user/proj", "src/"),
            "Explain src/ in /home/user/proj."
        );
    }

    #[test]
    fn expand_no_args_leaves_placeholder_empty() {
        let skill = Skill::new("/review", "review", "Review {dir} args={args}.");
        assert_eq!(skill.expand("/proj", ""), "Review /proj args=.");
    }

    // ── resolve ───────────────────────────────────────────────────────────────

    #[test]
    fn resolve_matches_command() {
        let skills = vec![Skill::new("/review", "review", "Review {dir}.")];
        let result = resolve(&skills, "/review", "/proj");
        assert_eq!(result, Some("Review /proj.".to_string()));
    }

    #[test]
    fn resolve_passes_args() {
        let skills = vec![Skill::new(
            "/explain",
            "explain",
            "Explain {args} in {dir}.",
        )];
        let result = resolve(&skills, "/explain src/lib.rs", "/proj");
        assert_eq!(result, Some("Explain src/lib.rs in /proj.".to_string()));
    }

    #[test]
    fn resolve_unknown_returns_none() {
        let skills = vec![Skill::new("/review", "review", "Review {dir}.")];
        assert!(resolve(&skills, "/unknown", "/proj").is_none());
    }

    #[test]
    fn resolve_plain_message_returns_none() {
        let skills = vec![Skill::new("/review", "review", "Review {dir}.")];
        assert!(resolve(&skills, "fix the bug", "/proj").is_none());
    }

    #[test]
    fn resolve_builtin_model_returns_none() {
        // /model is handled upstream, not by resolve.
        let skills: Vec<Skill> = vec![];
        assert!(resolve(&skills, "/model", "/proj").is_none());
    }

    // ── load_dir ──────────────────────────────────────────────────────────────

    #[test]
    fn load_dir_reads_md_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let skill_path = dir.path().join("myskill.md");
        std::fs::write(
            &skill_path,
            "---\ndescription = \"do a thing\"\n---\nDo {dir}.",
        )
        .expect("write");

        let mut map = std::collections::HashMap::new();
        load_dir(dir.path(), &mut map);

        assert!(map.contains_key("/myskill"));
        let skill = &map["/myskill"];
        assert_eq!(skill.description, "do a thing");
    }

    #[test]
    fn load_dir_ignores_non_md_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("notes.txt"), "not a skill").expect("write");

        let mut map = std::collections::HashMap::new();
        load_dir(dir.path(), &mut map);
        assert!(map.is_empty());
    }

    #[test]
    fn load_dir_missing_dir_is_no_op() {
        let mut map = std::collections::HashMap::new();
        load_dir(std::path::Path::new("/nonexistent/path/xyz"), &mut map);
        assert!(map.is_empty());
    }

    #[test]
    fn load_project_overrides_global() {
        let global_dir = tempfile::tempdir().expect("tempdir");
        let project_dir = tempfile::tempdir().expect("tempdir");

        std::fs::write(global_dir.path().join("review.md"), "global template")
            .expect("write global");
        std::fs::write(project_dir.path().join("review.md"), "project template")
            .expect("write project");

        let mut map = std::collections::HashMap::new();
        load_dir(global_dir.path(), &mut map);
        load_dir(project_dir.path(), &mut map);

        assert_eq!(map["/review"].template, "project template");
    }
}
