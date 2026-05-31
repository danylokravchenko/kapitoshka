use crossterm::{
    cursor, execute, queue,
    style::{Color, Print, ResetColor, SetForegroundColor},
};
use std::io::{self, Write as IoWrite};
use std::{thread, time::Duration};

fn stdout() -> io::Stdout {
    io::stdout()
}

// ── Animated banner ───────────────────────────────────────────────────────────

const ANIM_ROWS: u16 = 9;

// Each frame is 9 rows × ~38 cols.  Special chars are coloured by color_for_char.
// ·  ∘  ○ – rain drops (DarkGrey → Blue → Cyan)
// / \ | _ V – outline (BrightBlue)
// ◕ – eyes (Cyan)   ‿ – smile (White)
static FRAMES: &[&[&str]] = &[
    // 0 – rain at the top
    &[
        "  ·    ○    ·    ∘    ·    ○  ",
        "    ○    ·    ○    ·    ∘     ",
        "  ·    ∘    ·    ○    ·       ",
        "                              ",
        "                              ",
        "                              ",
        "                              ",
        "                              ",
        "                              ",
    ],
    // 1 – rain falls to the middle
    &[
        "                              ",
        "                              ",
        "  ○    ·    ○    ·    ∘    ·  ",
        "    ·    ○    ·    ○    ·     ",
        "  ○    ∘    ·    ○    ·       ",
        "                              ",
        "                              ",
        "                              ",
        "                              ",
    ],
    // 2 – drops converge toward centre
    &[
        "                              ",
        "                              ",
        "                              ",
        "    ○  ·  ○  ·  ○  ·  ○      ",
        "      ·  ○  ·  ○  ·  ○       ",
        "        ○  ·  ○  ·            ",
        "           ·  ○               ",
        "                              ",
        "                              ",
    ],
    // 3 – drops cluster into a drop outline
    &[
        "                              ",
        "                              ",
        "                              ",
        "                              ",
        "              ·               ",
        "           ○     ○            ",
        "          ○       ○           ",
        "           ○     ○            ",
        "              ○               ",
    ],
    // 4 – teardrop silhouette
    &[
        "                              ",
        "                              ",
        "              .               ",
        "             / \\             ",
        "            /   \\            ",
        "            |   |             ",
        "             \\ /             ",
        "              V               ",
        "                              ",
    ],
    // 5 – Kapitoshka face revealed
    &[
        "                              ",
        "              .               ",
        "             / \\             ",
        "            /   \\            ",
        "           / ◕ ◕ \\           ",
        "           |  ‿  |            ",
        "            \\   /            ",
        "             \\_/             ",
        "       kapitoshka agent       ",
    ],
];

fn color_for_char(ch: char, frame_idx: usize) -> Option<Color> {
    match ch {
        '·' => Some(Color::DarkGrey),
        '∘' => Some(Color::Blue),
        '○' => Some(if frame_idx.is_multiple_of(2) {
            Color::Cyan
        } else {
            Color::Blue
        }),
        '/' | '\\' | '|' | '_' => Some(Color::Rgb {
            r: 64,
            g: 180,
            b: 255,
        }),
        'V' => Some(Color::Blue),
        '.' => Some(Color::White),
        '◕' => Some(Color::Cyan),
        '‿' => Some(Color::White),
        _ => None,
    }
}

fn draw_anim_frame(out: &mut io::Stdout, lines: &[&str], frame_idx: usize) {
    for line in lines {
        for ch in line.chars() {
            if let Some(color) = color_for_char(ch, frame_idx) {
                let _ = queue!(out, SetForegroundColor(color), Print(ch), ResetColor);
            } else if frame_idx == FRAMES.len() - 1 {
                // last frame: colour the "kapitoshka agent" label cyan
                let _ = queue!(out, SetForegroundColor(Color::Cyan), Print(ch));
            } else {
                let _ = queue!(out, Print(ch));
            }
        }
        let _ = queue!(out, ResetColor, Print('\n'));
    }
    let _ = out.flush();
}

fn run_drop_animation(out: &mut io::Stdout) {
    // Reserve canvas
    for _ in 0..ANIM_ROWS {
        let _ = execute!(out, Print("\n"));
    }

    for (i, frame) in FRAMES.iter().enumerate() {
        let _ = execute!(out, cursor::MoveUp(ANIM_ROWS));
        draw_anim_frame(out, frame, i);
        let delay = if i + 1 == FRAMES.len() { 350 } else { 130 };
        thread::sleep(Duration::from_millis(delay));
    }
}

pub fn print_banner(model: &str, dir: &str, session_path: &str, thinking: bool) {
    let thinking_label = if thinking { "  (thinking on)\n" } else { "" };
    let mut out = stdout();

    let _ = execute!(out, cursor::Hide);
    run_drop_animation(&mut out);
    let _ = execute!(out, cursor::Show);

    let _ = execute!(
        out,
        Print(format!("  model : {model}\n")),
        Print(format!("  dir   : {dir}\n")),
        Print(format!("  log   : {session_path}\n")),
        SetForegroundColor(Color::DarkGrey),
        Print(thinking_label),
        Print("\n  Type your task and press Enter. Ctrl-D or 'exit' to quit.\n\n"),
        ResetColor,
    );
}

pub fn print_compacting() {
    let mut out = stdout();
    let _ = execute!(
        out,
        SetForegroundColor(Color::DarkGrey),
        Print("✂  compacting context…\n"),
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

/// Print the thinking block header.
pub fn stream_thinking_start() {
    let mut out = stdout();
    let _ = execute!(
        out,
        SetForegroundColor(Color::DarkGrey),
        Print("\n💭 thinking\n"),
    );
}

/// Print the `│ ` line prefix at the start of a new thinking line.
pub fn stream_thinking_prefix() {
    let mut out = stdout();
    let _ = queue!(out, SetForegroundColor(Color::DarkGrey), Print("│ "));
    let _ = out.flush();
}

/// Stream a raw chunk of thinking text (no prefix, no newline added), flushing immediately.
pub fn stream_thinking_chunk(chunk: &str) {
    let mut out = stdout();
    let _ = queue!(out, SetForegroundColor(Color::DarkGrey), Print(chunk));
    let _ = out.flush();
}

/// Close the thinking block with a separator line.
pub fn stream_thinking_end() {
    let mut out = stdout();
    let _ = execute!(
        out,
        Print("\n"),
        SetForegroundColor(Color::DarkGrey),
        Print("└─────────────────────────────────────\n"),
        ResetColor,
    );
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

#[allow(clippy::too_many_arguments)]
pub fn print_context_stats(
    input_tokens: u64,
    output_tokens: u64,
    total_tokens: u64,
    cached: u64,
    reasoning: u64,
    last_input_tokens: u64,
    context_size: u64,
    compacted: bool,
) {
    let mut out = stdout();
    let compacted_label = if compacted {
        "  ✂ history compacted\n"
    } else {
        ""
    };
    let _ = execute!(
        out,
        SetForegroundColor(Color::DarkGrey),
        Print(compacted_label),
        Print(format!(
            "  ctx  in:{input_tokens}  out:{output_tokens}  total:{total_tokens}"
        )),
    );
    if cached > 0 {
        let _ = execute!(out, Print(format!("  cached:{cached}")));
    }
    if reasoning > 0 {
        let _ = execute!(out, Print(format!("  think:{reasoning}")));
    }
    if context_size > 0 && last_input_tokens > 0 {
        let pct = last_input_tokens * 100 / context_size;
        let size_k = context_size / 1000;
        let _ = execute!(out, Print(format!("  {pct}% of {size_k}k")));
    }
    let _ = execute!(out, ResetColor, Print("\n"));
}

pub fn print_todo_list(todos: &[crate::tools::todo::TodoItem]) {
    use crate::tools::todo::TodoStatus;
    let mut out = stdout();
    let _ = execute!(
        out,
        SetForegroundColor(Color::Cyan),
        Print("\n  Plan\n  ────\n"),
        ResetColor,
    );
    for item in todos {
        let (icon, color) = match item.status {
            TodoStatus::Pending => ("☐", Color::DarkGrey),
            TodoStatus::InProgress => ("⟳", Color::Yellow),
            TodoStatus::Completed => ("✓", Color::Green),
        };
        let _ = execute!(
            out,
            SetForegroundColor(color),
            Print(format!("  {icon} {}\n", item.content)),
            ResetColor,
        );
    }
    let _ = execute!(out, Print("\n"));
}

pub fn print_subagent_start(task: &str) {
    let preview: String = task.chars().take(72).collect();
    let ellipsis = if task.chars().count() > 72 { "…" } else { "" };
    let mut out = stdout();
    let _ = execute!(
        out,
        SetForegroundColor(Color::Cyan),
        Print(format!("  ⇢  subagent: {preview}{ellipsis}\n")),
        ResetColor,
    );
}

pub fn print_subagent_done() {
    let mut out = stdout();
    let _ = execute!(
        out,
        SetForegroundColor(Color::DarkGrey),
        Print("  ⇠  subagent done\n"),
        ResetColor,
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

/// Called when the user cancels a turn mid-stream with Ctrl+C.
pub fn print_model_list(models: &[String], current: &str) {
    let mut out = stdout();
    let _ = execute!(
        out,
        SetForegroundColor(Color::Cyan),
        Print("\n  Available models\n  ─────────────────\n"),
        ResetColor,
    );
    for (i, m) in models.iter().enumerate() {
        let marker = if m == current { " ◀" } else { "" };
        let color = if m == current {
            Color::White
        } else {
            Color::DarkGrey
        };
        let _ = execute!(
            out,
            SetForegroundColor(color),
            Print(format!("  [{:>2}] {m}{marker}\n", i + 1)),
            ResetColor,
        );
    }
    let _ = execute!(out, Print("\n"));
}

/// Compact header shown when switching models mid-session (no animation).
pub fn print_model_switch(model: &str, dir: &str, session_path: &str, thinking: bool) {
    let thinking_label = if thinking { "  (thinking on)\n" } else { "" };
    let mut out = stdout();
    let _ = execute!(
        out,
        Print("\n"),
        SetForegroundColor(Color::Cyan),
        Print(format!("  ─── switched to {model} ───\n")),
        ResetColor,
        SetForegroundColor(Color::DarkGrey),
        Print(format!("  dir   : {dir}\n")),
        Print(format!("  log   : {session_path}\n")),
        Print(thinking_label),
        Print("\n"),
        ResetColor,
    );
}

pub fn print_model_changed(model: &str) {
    let mut out = stdout();
    let _ = execute!(
        out,
        SetForegroundColor(Color::Cyan),
        Print(format!("  ✓  model → {model}\n")),
        ResetColor,
    );
}

pub fn print_cancelled() {
    let mut out = stdout();
    let _ = execute!(
        out,
        ResetColor,
        Print("\n"),
        SetForegroundColor(Color::DarkGrey),
        Print("  ⊘  cancelled\n"),
        ResetColor,
    );
}
