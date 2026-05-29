use crossterm::{
    execute, queue,
    style::{Color, Print, ResetColor, SetForegroundColor},
};
use std::io::{self, Write as IoWrite};

fn stdout() -> io::Stdout {
    io::stdout()
}

pub fn print_banner(model: &str, dir: &str, session_path: &str, thinking: bool) {
    let thinking_label = if thinking { "  (thinking on)\n" } else { "" };
    let mut out = stdout();
    let _ = execute!(
        out,
        SetForegroundColor(Color::Cyan),
        Print("╔══════════════════════════════════════╗\n"),
        Print("║          kapitoshka agent            ║\n"),
        Print("╚══════════════════════════════════════╝\n"),
        ResetColor,
        Print(format!("  model : {model}\n")),
        Print(format!("  dir   : {dir}\n")),
        Print(format!("  log   : {session_path}\n")),
        SetForegroundColor(Color::DarkGrey),
        Print(thinking_label),
        Print("\n  Type your task and press Enter. Ctrl-D or 'exit' to quit.\n\n"),
        ResetColor,
    );
}

pub fn print_working() {
    let mut out = stdout();
    let _ = execute!(
        out,
        SetForegroundColor(Color::DarkGrey),
        Print("⟳  working…\n"),
        ResetColor,
    );
}

/// Called by tools to announce what they are about to do.
pub fn print_tool_action(tool: &str, action: &str) {
    let mut out = stdout();
    let _ = execute!(
        out,
        SetForegroundColor(Color::Yellow),
        Print(format!("  → {tool}")),
        ResetColor,
        Print(format!(": {action}\n")),
    );
}

/// Show a [y/N] permission prompt and return whether the user approved.
/// Blocks until the user presses Enter.
pub fn ask_permission(tool: &str, action: &str) -> bool {
    let mut out = stdout();
    let _ = execute!(
        out,
        SetForegroundColor(Color::Magenta),
        Print(format!("  ⚠  {tool}: {action}\n")),
        Print("     Allow? [y/N] "),
        ResetColor,
    );
    let _ = out.flush();

    let mut input = String::new();
    io::stdin().read_line(&mut input).unwrap_or(0);
    matches!(input.trim().to_lowercase().as_str(), "y" | "yes")
}

// ── Streaming output ──────────────────────────────────────────────────────────

/// Print the opening newline before the first streamed text chunk.
pub fn stream_response_start() {
    let mut out = stdout();
    let _ = execute!(out, Print("\n"), SetForegroundColor(Color::White));
}

/// Stream a text chunk to the terminal, flushing immediately.
pub fn stream_text(chunk: &str) {
    let mut out = stdout();
    let _ = queue!(out, Print(chunk));
    let _ = out.flush();
}

/// Mark the end of the streamed response (newline + colour reset).
pub fn stream_response_end() {
    let mut out = stdout();
    let _ = execute!(out, ResetColor, Print("\n"));
}

/// Print the "💭 thinking:" header before reasoning chunks.
pub fn stream_thinking_start() {
    let mut out = stdout();
    let _ = execute!(
        out,
        SetForegroundColor(Color::DarkGrey),
        Print("\n💭 thinking:\n"),
    );
}

/// Stream a thinking/reasoning chunk, flushing immediately.
pub fn stream_thinking(chunk: &str) {
    let mut out = stdout();
    let _ = queue!(out, SetForegroundColor(Color::DarkGrey), Print(chunk));
    let _ = out.flush();
}

/// Close the thinking section.
pub fn stream_thinking_end() {
    let mut out = stdout();
    let _ = execute!(out, ResetColor, Print("\n"));
}

// ── Non-streaming fallback ────────────────────────────────────────────────────

pub fn print_response(text: &str) {
    let mut out = stdout();
    let _ = execute!(
        out,
        Print("\n"),
        SetForegroundColor(Color::White),
        Print(text),
        ResetColor,
        Print("\n"),
    );
}

pub fn print_error(msg: &str) {
    let mut out = stdout();
    let _ = execute!(
        out,
        SetForegroundColor(Color::Red),
        Print(format!("  ✗  {msg}\n")),
        ResetColor,
    );
}
