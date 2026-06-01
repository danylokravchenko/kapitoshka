/// Hard caps applied by each squasher.
const MAX_LINES: usize = 200;
const MAX_DIFF_LINES: usize = 300;
const MAX_FIND_ENTRIES: usize = 100;
const MAX_GREP_MATCHES: usize = 100;

// ── Public API ────────────────────────────────────────────────────────────────

/// Squash shell output before it enters the agent's context window.
///
/// Detects the command type and applies the most aggressive lossless-to-meaning
/// compressor available. Falls back to a generic compressor (ANSI strip +
/// run-length collapse + line cap) for unrecognised commands.
///
/// Returns `(squashed_stdout, squashed_stderr)`.
pub fn squash(command: &str, stdout: &str, stderr: &str) -> (String, String) {
    let cmd = normalise(command);
    let first = cmd.split_whitespace().next().unwrap_or("");

    match first {
        "cargo" => (squash_cargo(stdout, stderr), String::new()),
        "git" => squash_git(&cmd, stdout, stderr),
        "find" | "ls" => (squash_find(&join(stdout, stderr)), String::new()),
        "grep" | "rg" | "ag" | "ripgrep" => (squash_grep(&join(stdout, stderr)), String::new()),
        _ => (generic(&strip_ansi(stdout)), generic(&strip_ansi(stderr))),
    }
}

// ── Per-command squashers ─────────────────────────────────────────────────────

/// Cargo: strip progress noise, collapse passing tests, keep signal.
fn squash_cargo(stdout: &str, stderr: &str) -> String {
    let combined = strip_ansi(&join(stdout, stderr));
    let mut out: Vec<String> = Vec::new();
    let mut pass_count: usize = 0;

    for line in combined.lines() {
        let t = line.trim_start();

        // `test module::name ... ok`  — collapse into a count
        if is_passing_test(t) {
            pass_count += 1;
            continue;
        }

        // Flush accumulated pass count before any other signal line
        if pass_count > 0 {
            out.push(format!("({pass_count} tests passed)"));
            pass_count = 0;
        }

        if is_cargo_signal(t) {
            out.push(line.to_owned());
        }
        // All other lines (Compiling, Downloading, Finished, Checking…) are dropped.
    }

    if pass_count > 0 {
        out.push(format!("({pass_count} tests passed)"));
    }

    if out.is_empty() {
        return "(no errors or warnings)".to_owned();
    }

    cap_lines(&out.join("\n"), MAX_LINES)
}

fn is_passing_test(t: &str) -> bool {
    t.starts_with("test ") && (t.ends_with("... ok") || t.ends_with(" ok"))
}

fn is_cargo_signal(t: &str) -> bool {
    t.starts_with("error")
        || t.starts_with("warning")
        || t.starts_with("note")
        || t.starts_with("help")
        || t.starts_with("-->")
        || t.starts_with('|')
        || t.starts_with('=')
        || t.starts_with("thread '")
        || t.starts_with("FAILED")
        || t.starts_with("test result")
        || t.starts_with("failures:")
        || t.starts_with("running ")
        || t.contains("error[")
        || t.contains("warning[")
}

/// Git: diffs get a generous line cap; everything else gets generic treatment.
fn squash_git(cmd: &str, stdout: &str, stderr: &str) -> (String, String) {
    let words: Vec<&str> = cmd.split_whitespace().collect();
    let sub = words.get(1).copied().unwrap_or("");
    match sub {
        "diff" | "show" | "blame" => {
            let out = cap_lines(&strip_ansi(&join(stdout, stderr)), MAX_DIFF_LINES);
            (out, String::new())
        }
        _ => (generic(&strip_ansi(stdout)), generic(&strip_ansi(stderr))),
    }
}

/// Find/ls: strip ANSI and cap at MAX_FIND_ENTRIES.
fn squash_find(text: &str) -> String {
    cap_lines(&strip_ansi(text), MAX_FIND_ENTRIES)
}

/// Grep/rg: strip ANSI and cap at MAX_GREP_MATCHES.
fn squash_grep(text: &str) -> String {
    cap_lines(&strip_ansi(text), MAX_GREP_MATCHES)
}

/// Generic: strip ANSI + collapse identical adjacent lines + cap.
fn generic(text: &str) -> String {
    cap_lines(&collapse_runs(text), MAX_LINES)
}

// ── Primitives ────────────────────────────────────────────────────────────────

/// Collapse consecutive identical lines into `line (×N)`.
pub fn collapse_runs(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut prev: Option<&str> = None;
    let mut count: usize = 0;

    for line in text.lines() {
        if Some(line) == prev {
            count += 1;
        } else {
            if let Some(p) = prev {
                push_run(&mut out, p, count);
            }
            prev = Some(line);
            count = 1;
        }
    }
    if let Some(p) = prev {
        push_run(&mut out, p, count);
    }
    out.join("\n")
}

fn push_run(out: &mut Vec<String>, line: &str, count: usize) {
    if count > 1 {
        out.push(format!("{line} (×{count})"));
    } else {
        out.push(line.to_owned());
    }
}

/// Truncate to `max` lines, appending `[... N lines omitted]` when cut.
pub fn cap_lines(text: &str, max: usize) -> String {
    if text.is_empty() {
        return String::new();
    }
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= max {
        return text.to_owned();
    }
    format!(
        "{}\n[... {} lines omitted]",
        lines[..max].join("\n"),
        lines.len() - max
    )
}

/// Strip ANSI/VT100 escape sequences (e.g. colour codes).
pub fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' && chars.peek() == Some(&'[') {
            chars.next(); // consume '['
            // Consume parameter bytes and the final command byte
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(ch);
        }
    }
    out
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn normalise(cmd: &str) -> String {
    cmd.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn join(stdout: &str, stderr: &str) -> String {
    match (stdout.is_empty(), stderr.is_empty()) {
        (true, _) => stderr.to_owned(),
        (_, true) => stdout.to_owned(),
        _ => format!("{stdout}{stderr}"),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── strip_ansi ────────────────────────────────────────────────────────────

    #[test]
    fn strip_ansi_removes_colour_codes() {
        let input = "\x1b[32mhello\x1b[0m world";
        assert_eq!(strip_ansi(input), "hello world");
    }

    #[test]
    fn strip_ansi_passthrough_plain_text() {
        let input = "plain text";
        assert_eq!(strip_ansi(input), "plain text");
    }

    // ── collapse_runs ─────────────────────────────────────────────────────────

    #[test]
    fn collapse_runs_no_repeats() {
        let text = "a\nb\nc";
        assert_eq!(collapse_runs(text), "a\nb\nc");
    }

    #[test]
    fn collapse_runs_three_identical() {
        let text = "foo\nfoo\nfoo";
        assert_eq!(collapse_runs(text), "foo (×3)");
    }

    #[test]
    fn collapse_runs_mixed() {
        let text = "a\nb\nb\nc";
        assert_eq!(collapse_runs(text), "a\nb (×2)\nc");
    }

    // ── cap_lines ─────────────────────────────────────────────────────────────

    #[test]
    fn cap_lines_under_limit() {
        let text = "a\nb\nc";
        assert_eq!(cap_lines(text, 10), text);
    }

    #[test]
    fn cap_lines_at_limit_appends_note() {
        let lines: Vec<String> = (0..15).map(|i| format!("line {i}")).collect();
        let text = lines.join("\n");
        let out = cap_lines(&text, 10);
        assert!(out.contains("[... 5 lines omitted]"));
        assert_eq!(out.lines().count(), 11); // 10 kept + trailer
    }

    // ── squash_cargo ──────────────────────────────────────────────────────────

    #[test]
    fn cargo_strips_compiling_lines() {
        let stderr = "   Compiling foo v0.1.0\n   Compiling bar v1.2.3\n    Finished dev\n";
        let out = squash_cargo("", stderr);
        assert_eq!(out, "(no errors or warnings)");
    }

    #[test]
    fn cargo_keeps_errors_and_warnings() {
        let stderr = concat!(
            "   Compiling foo v0.1.0\n",
            "error[E0308]: mismatched types\n",
            " --> src/main.rs:5:10\n",
            "  |\n",
            "5 |     let x: i32 = \"hello\";\n",
            "  |                  ^^^^^^^ expected `i32`, found `&str`\n",
            "warning: unused variable `y`\n",
            "    Finished dev\n",
        );
        let out = squash_cargo("", stderr);
        assert!(out.contains("error[E0308]"));
        assert!(out.contains("warning: unused variable"));
        assert!(!out.contains("Compiling"));
        assert!(!out.contains("Finished"));
    }

    #[test]
    fn cargo_collapses_passing_tests() {
        let stdout = concat!(
            "running 5 tests\n",
            "test a::foo ... ok\n",
            "test a::bar ... ok\n",
            "test b::baz ... ok\n",
            "test result: ok. 3 passed; 0 failed\n",
        );
        let out = squash_cargo(stdout, "");
        assert!(out.contains("(3 tests passed)"));
        assert!(!out.contains("test a::foo"));
        assert!(out.contains("test result: ok"));
    }

    #[test]
    fn cargo_keeps_failed_tests_inline() {
        let stdout = concat!(
            "running 3 tests\n",
            "test ok_1 ... ok\n",
            "test bad  ... FAILED\n",
            "test ok_2 ... ok\n",
            "test result: FAILED. 2 passed; 1 failed\n",
        );
        let out = squash_cargo(stdout, "");
        assert!(out.contains("FAILED"));
        assert!(out.contains("test result: FAILED"));
        assert!(!out.contains("test ok_1"));
        assert!(!out.contains("test ok_2"));
    }

    // ── squash dispatch ───────────────────────────────────────────────────────

    #[test]
    fn dispatch_cargo_build() {
        let (out, err) = squash("cargo build", "", "   Compiling x v0.1\n    Finished dev\n");
        assert_eq!(out, "(no errors or warnings)");
        assert!(err.is_empty());
    }

    #[test]
    fn dispatch_find_caps_entries() {
        let lines: String = (0..150).map(|i| format!("./file{i}.rs\n")).collect();
        let (out, _) = squash("find . -name '*.rs'", &lines, "");
        assert!(out.contains("lines omitted"));
    }

    #[test]
    fn dispatch_grep_caps_matches() {
        let lines: String = (0..120)
            .map(|i| format!("src/foo.rs:{i}: match\n"))
            .collect();
        let (out, _) = squash("grep -r pattern src/", &lines, "");
        assert!(out.contains("lines omitted"));
    }

    #[test]
    fn dispatch_generic_collapses_runs() {
        let text = "same line\nsame line\nsame line\n";
        let (out, _) = squash("echo test", text, "");
        assert!(out.contains("×3"));
    }

    #[test]
    fn dispatch_generic_strips_ansi() {
        let text = "\x1b[32mcoloured\x1b[0m";
        let (out, _) = squash("some_tool", text, "");
        assert_eq!(out.trim(), "coloured");
    }
}
