use rustyline::completion::{Completer, Pair};
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::Validator;
use rustyline::{Context, Helper};
use std::borrow::Cow;

use crate::skills::Skill;

// ── Built-in commands always available for completion ─────────────────────────

const BUILTIN_COMMANDS: &[(&str, &str)] = &[("/model", "switch the active model")];

// ── Helper ────────────────────────────────────────────────────────────────────

/// rustyline `Helper` that provides:
/// - Tab-completion for `/skill` commands and `/model`.
/// - Inline ghost-text hint for the first matching command.
/// - Dim-grey colouring of hints; green colouring of `/` commands.
pub struct KapHelper {
    skills: Vec<Skill>,
}

impl KapHelper {
    pub fn new(skills: Vec<Skill>) -> Self {
        Self { skills }
    }

    /// Iterate over all known commands (built-ins + loaded skills).
    fn all_commands(&self) -> impl Iterator<Item = (&str, &str)> {
        let builtins = BUILTIN_COMMANDS.iter().map(|(name, desc)| (*name, *desc));
        let loaded = self
            .skills
            .iter()
            .map(|s| (s.name.as_str(), s.description.as_str()));
        builtins.chain(loaded)
    }
}

// ── Completer ─────────────────────────────────────────────────────────────────

impl Completer for KapHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        // Only complete slash commands typed at the very start of the line.
        let word = &line[..pos];
        if !word.starts_with('/') {
            return Ok((pos, vec![]));
        }
        let candidates: Vec<Pair> = self
            .all_commands()
            .filter(|(name, _)| name.starts_with(word))
            .map(|(name, desc)| Pair {
                display: format!("{name}  \x1b[90m{desc}\x1b[0m"),
                replacement: name.to_string(),
            })
            .collect();
        Ok((0, candidates))
    }
}

// ── Hinter ────────────────────────────────────────────────────────────────────

pub struct CommandHint(String);

impl rustyline::hint::Hint for CommandHint {
    fn display(&self) -> &str {
        &self.0
    }
    fn completion(&self) -> Option<&str> {
        Some(&self.0)
    }
}

impl Hinter for KapHelper {
    type Hint = CommandHint;

    fn hint(&self, line: &str, pos: usize, _ctx: &Context<'_>) -> Option<CommandHint> {
        // Show a hint only when the cursor is at the end and line starts with `/`.
        if pos < line.len() || !line.starts_with('/') {
            return None;
        }
        self.all_commands()
            .find(|(name, _)| name.starts_with(line) && *name != line)
            .map(|(name, _)| CommandHint(name[line.len()..].to_string()))
    }
}

// ── Highlighter ───────────────────────────────────────────────────────────────

impl Highlighter for KapHelper {
    fn highlight_hint<'h>(&self, hint: &'h str) -> Cow<'h, str> {
        Cow::Owned(format!("\x1b[90m{hint}\x1b[0m"))
    }

    fn highlight<'l>(&self, line: &'l str, _pos: usize) -> Cow<'l, str> {
        if line.starts_with('/') {
            Cow::Owned(format!("\x1b[32m{line}\x1b[0m"))
        } else {
            Cow::Borrowed(line)
        }
    }

    fn highlight_char(
        &self,
        line: &str,
        _pos: usize,
        _forced: rustyline::highlight::CmdKind,
    ) -> bool {
        line.starts_with('/')
    }
}

// ── Validator ─────────────────────────────────────────────────────────────────

impl Validator for KapHelper {}

// ── Helper blanket ────────────────────────────────────────────────────────────

impl Helper for KapHelper {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::Skill;
    use rustyline::Context;
    use rustyline::hint::Hint as _;
    use rustyline::history::DefaultHistory;

    fn ctx() -> Context<'static> {
        // Context::new requires a History reference; we use a leaked default one
        // so the borrow lives long enough for the test call.
        let history: &'static DefaultHistory = Box::leak(Box::new(DefaultHistory::new()));
        Context::new(history)
    }

    fn helper(names: &[&str]) -> KapHelper {
        let skills = names
            .iter()
            .map(|n| Skill::new(*n, format!("{n} description"), "template"))
            .collect();
        KapHelper::new(skills)
    }

    // ── Completer ─────────────────────────────────────────────────────────────

    #[test]
    fn complete_empty_line_returns_nothing() {
        let h = helper(&["/review"]);
        let (_, candidates) = h.complete("", 0, &ctx()).expect("complete");
        assert!(candidates.is_empty());
    }

    #[test]
    fn complete_plain_text_returns_nothing() {
        let h = helper(&["/review"]);
        let (_, candidates) = h.complete("fix the bug", 11, &ctx()).expect("complete");
        assert!(candidates.is_empty());
    }

    #[test]
    fn complete_slash_alone_returns_all_commands() {
        let h = helper(&["/review", "/verify"]);
        let (_, candidates) = h.complete("/", 1, &ctx()).expect("complete");
        // Built-in /model + 2 skills = 3
        assert_eq!(candidates.len(), 3);
    }

    #[test]
    fn complete_partial_prefix_filters_candidates() {
        let h = helper(&["/review", "/verify", "/explain"]);
        let (_, candidates) = h.complete("/ve", 3, &ctx()).expect("complete");
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].replacement, "/verify");
    }

    #[test]
    fn complete_exact_match_still_returned() {
        let h = helper(&["/review"]);
        let (_, candidates) = h.complete("/review", 7, &ctx()).expect("complete");
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].replacement, "/review");
    }

    #[test]
    fn complete_replacement_starts_at_zero() {
        let h = helper(&["/review"]);
        let (start, _) = h.complete("/re", 3, &ctx()).expect("complete");
        assert_eq!(start, 0);
    }

    #[test]
    fn complete_builtin_model_always_present() {
        let h = helper(&[]);
        let (_, candidates) = h.complete("/m", 2, &ctx()).expect("complete");
        assert!(candidates.iter().any(|c| c.replacement == "/model"));
    }

    // ── Hinter ────────────────────────────────────────────────────────────────

    #[test]
    fn hint_plain_text_returns_none() {
        let h = helper(&["/review"]);
        assert!(h.hint("fix bug", 7, &ctx()).is_none());
    }

    #[test]
    fn hint_cursor_not_at_end_returns_none() {
        let h = helper(&["/review"]);
        // cursor at pos 1 while line is longer
        assert!(h.hint("/review", 1, &ctx()).is_none());
    }

    #[test]
    fn hint_partial_command_returns_suffix() {
        let h = helper(&["/review"]);
        let hint = h.hint("/rev", 4, &ctx()).expect("hint");
        assert_eq!(hint.display(), "iew");
    }

    #[test]
    fn hint_exact_command_returns_none() {
        let h = helper(&["/review"]);
        assert!(h.hint("/review", 7, &ctx()).is_none());
    }

    #[test]
    fn hint_completion_matches_display() {
        let h = helper(&["/verify"]);
        let hint = h.hint("/ver", 4, &ctx()).expect("hint");
        assert_eq!(hint.completion(), Some("ify"));
    }

    #[test]
    fn hint_slash_alone_hints_first_alphabetical() {
        // Built-in /model sorts before /review alphabetically.
        let h = helper(&["/review"]);
        let hint = h.hint("/", 1, &ctx()).expect("hint");
        // The first match from builtins is "/model" → suffix "model"
        assert_eq!(hint.display(), "model");
    }
}
