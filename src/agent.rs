use anyhow::Result;
use futures::StreamExt;
use rig::agent::{Agent, FinalResponse, MultiTurnStreamItem};
use rig::client::{CompletionClient, ProviderClient};
use rig::completion::{CompletionModel, Message, Usage};
use rig::providers::{anthropic, openai};
use rig::streaming::{StreamedAssistantContent, StreamingChat};
use rustyline::DefaultEditor;
use rustyline::error::ReadlineError;
use std::time::Duration;
use tracing::Instrument as _;

use crate::context;
use crate::models;
use crate::permission;
use crate::session::Session;
use crate::settings::Settings;
use crate::tools::all_tools;
use crate::trajectory::TrajectoryRecorder;
use crate::ui;

/// Tracks cumulative token usage across all turns in a session.
#[derive(Debug, Default, Clone, Copy)]
pub struct ContextStats {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub cached_input_tokens: u64,
    pub reasoning_tokens: u64,
}

impl ContextStats {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, usage: &Usage) {
        self.input_tokens += usage.input_tokens;
        self.output_tokens += usage.output_tokens;
        self.total_tokens += usage.total_tokens;
        self.cached_input_tokens += usage.cached_input_tokens;
        self.reasoning_tokens += usage.reasoning_tokens;
    }
}

/// Maximum wall-clock time allowed for a single agent turn.
const TURN_TIMEOUT: Duration = Duration::from_secs(300);

const SYSTEM_PROMPT_BASE: &str = "\
You are kapitoshka, an expert coding agent. You help users with software engineering tasks.

You have access to tools to read files, write files, list directories, run shell commands,
and manage a structured task plan via `todo_write`.

## Planning

For any non-trivial task (more than a single lookup or one-line fix):
1. Invoke the `todo_write` tool immediately with a numbered plan before doing any other work.
2. As you start each step, invoke `todo_write` again to mark it `in_progress`.
3. When a step is done, mark it `completed` before moving to the next one.
4. If you discover the plan needs revision mid-task, invoke `todo_write` with an updated list.

For simple questions or tiny one-step tasks you may skip planning.

## Delegation

Use `spawn_agent` tool to delegate self-contained sub-tasks to a focused subagent. Good candidates:
- Investigating or summarising a specific module or file set
- Performing a scoped rewrite or refactor on a single file
- Running tests and summarising failures
- Any step whose inputs and expected output can be described completely in one prompt

Rules for effective delegation:
1. The subagent has no memory of this conversation — put everything it needs in `task`.
2. Include relevant file paths, goals, and constraints explicitly.
3. Use `context` for role framing or additional constraints on the subagent.
4. Do not delegate tasks that require back-and-forth with the user.
5. Subagents cannot spawn further subagents; keep delegated tasks atomic.

## File operations

Always explore the codebase before making changes. Prefer targeted edits over full rewrites.
After making changes, verify them by reading the modified files or running relevant commands.

Working directory is provided in the task. Use it as the root for all file operations.";

/// Build the system prompt, appending AGENTS.md from `dir` if it exists.
///
/// AGENTS.md may contain agent-specific sections introduced by a heading whose
/// text matches the agent name (case-insensitive), e.g. `## kapitoshka`.
/// We include: all content before the first agent-specific heading (global
/// rules), plus the body of the section that matches `agent_name` (if any).
/// Sections for other agents are omitted.
fn build_system_prompt(dir: &str) -> String {
    let agents_md = std::path::Path::new(dir).join("AGENTS.md");
    let contents = match std::fs::read_to_string(&agents_md) {
        Ok(c) if !c.trim().is_empty() => c,
        Ok(_) => return SYSTEM_PROMPT_BASE.to_string(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return SYSTEM_PROMPT_BASE.to_string();
        }
        Err(e) => {
            tracing::warn!(path = %agents_md.display(), error = %e, "failed to read AGENTS.md");
            return SYSTEM_PROMPT_BASE.to_string();
        }
    };

    let rules = extract_agent_rules(&contents, "kapitoshka");
    if rules.trim().is_empty() {
        return SYSTEM_PROMPT_BASE.to_string();
    }
    tracing::info!(path = %agents_md.display(), "loaded AGENTS.md");
    format!("{SYSTEM_PROMPT_BASE}\n\n# Project Rules (AGENTS.md)\n\n{rules}")
}

/// Extract the rules from `contents` that apply to `agent_name`:
/// - All text before the first agent-specific heading (global rules).
/// - The body of the heading section whose title matches `agent_name` (case-insensitive).
///
/// An "agent-specific heading" is any ATX heading (`#`…`######`) whose text,
/// when compared case-insensitively and with surrounding whitespace stripped,
/// matches a known agent name. We identify them by checking every heading
/// against `agent_name`; headings that do NOT match are treated as agent sections
/// to skip, and headings that DO match are the section to include.
fn extract_agent_rules(contents: &str, agent_name: &str) -> String {
    let agent_name_lower = agent_name.to_lowercase();

    // Split into lines and iterate, tracking which section we are in.
    enum Section {
        Global, // before any agent heading
        Ours,   // inside the matching agent's section
        Other,  // inside a different agent's section
    }

    let mut section = Section::Global;
    let mut global = String::new();
    let mut ours = String::new();

    for line in contents.lines() {
        // Check if line is an ATX heading.
        let trimmed = line.trim_start_matches('#');
        let hashes = line.len() - trimmed.len();
        if hashes > 0 && hashes <= 6 && trimmed.starts_with(' ') {
            let heading_text = trimmed.trim().to_lowercase();
            if heading_text == agent_name_lower {
                section = Section::Ours;
                continue; // drop the heading itself
            } else {
                // Any other heading — could be a sub-heading inside our section
                // or a sibling agent section. We treat top-level headings (#, ##)
                // as potential agent-section delimiters; deeper ones stay in context.
                if hashes <= 2 {
                    section = Section::Other;
                    continue;
                }
                // Deeper heading: stays inside whatever section we're already in.
            }
        }

        match section {
            Section::Global => {
                global.push_str(line);
                global.push('\n');
            }
            Section::Ours => {
                ours.push_str(line);
                ours.push('\n');
            }
            Section::Other => {}
        }
    }

    let global = global.trim_end().to_string();
    let ours = ours.trim_end().to_string();

    match (global.is_empty(), ours.is_empty()) {
        (true, true) => String::new(),
        (false, true) => global,
        (true, false) => ours,
        (false, false) => format!("{global}\n\n{ours}"),
    }
}

/// Resolve a raw selection string (number or name) against a model list.
/// Returns `None` if the selection is out of range or not found.
fn resolve_model_selection(sel: &str, available: &[String]) -> Option<String> {
    if let Ok(n) = sel.parse::<usize>() {
        available.get(n.saturating_sub(1)).cloned()
    } else if available.iter().any(|m| m == sel) {
        Some(sel.to_string())
    } else {
        None
    }
}

/// Prompt the user to pick one model from the list using stdin directly.
/// Used at startup before the rustyline loop is running.
fn pick_model_interactive(available: &[String]) -> Result<String> {
    use std::io::BufRead as _;
    use std::io::Write as _;
    let mut out = std::io::stdout();
    write!(out, "\x1b[32m  select (1-N or name): \x1b[0m")?;
    out.flush()?;
    let line = std::io::BufReader::new(std::io::stdin())
        .lines()
        .next()
        .transpose()?
        .unwrap_or_default();
    resolve_model_selection(line.trim(), available)
        .ok_or_else(|| anyhow::anyhow!("invalid selection '{}'", line.trim()))
}

/// Fetch models, show the list, and read a selection from `rl`.
/// Returns `Some(new_model)` if a *different* model was chosen, `None` otherwise.
async fn handle_model_command(
    provider: &str,
    current: &str,
    rl: &mut DefaultEditor,
) -> Result<Option<String>> {
    let available = match models::fetch_models(provider).await {
        Ok(m) if m.is_empty() => {
            ui::print_error("no models returned by the inference engine");
            return Ok(None);
        }
        Ok(m) => m,
        Err(e) => {
            ui::print_error(&format!("failed to fetch models: {e}"));
            return Ok(None);
        }
    };

    ui::print_model_list(&available, current);
    let prompt = "\x01\x1b[32m\x02  select (1-N or name): \x01\x1b[0m\x02";
    let sel = match rl.readline(prompt) {
        Ok(s) => s,
        Err(_) => return Ok(None),
    };

    match resolve_model_selection(sel.trim(), &available) {
        Some(m) if m != current => Ok(Some(m)),
        Some(_) => Ok(None), // same model selected — no-op
        None => {
            ui::print_error("invalid selection");
            Ok(None)
        }
    }
}

pub async fn run_interactive(
    dir: &str,
    model: &str,
    provider: &str,
    thinking: bool,
    context_size: u64,
    session_id: &str,
    resume: Option<&std::path::Path>,
) -> Result<()> {
    let session_span = tracing::info_span!("session", id = %session_id, dir = %dir, model = %model);

    // Load prior history + scratchpad when resuming a crashed or previous session.
    let (mut history, mut scratchpad) = match resume {
        Some(path) => {
            let (h, s) = Session::load_state(path)?;
            tracing::info!(path = %path.display(), turns = h.len(), "resumed session state");
            (h, s)
        }
        None => (Vec::new(), String::new()),
    };

    // If no model is saved yet, fetch the list and let the user pick before starting.
    let mut current_model = if model.is_empty() {
        match models::fetch_models(provider).await {
            Ok(available) if !available.is_empty() => {
                ui::print_model_list(&available, "");
                pick_model_interactive(&available)?
            }
            Ok(_) => {
                anyhow::bail!("inference engine returned no models — set a model and retry");
            }
            Err(e) => {
                anyhow::bail!("failed to fetch models: {e}");
            }
        }
    } else {
        model.to_string()
    };

    let mut session = Session::new(dir, &current_model)?;
    let mut first_start = true;

    loop {
        let system_prompt = build_system_prompt(dir);

        let new_model = match provider {
            "anthropic" => {
                let client = anthropic::Client::from_env()?;
                let perm = permission::interactive();
                let agent = client
                    .agent(current_model.as_str())
                    .preamble(&system_prompt)
                    .tools(all_tools(dir, perm, &current_model, provider, thinking))
                    .max_tokens(8192)
                    .default_max_turns(20)
                    .build();
                run_loop(
                    agent,
                    client,
                    dir,
                    &current_model,
                    provider,
                    thinking,
                    context_size,
                    &mut session,
                    &mut history,
                    &mut scratchpad,
                    first_start,
                )
                .instrument(session_span.clone())
                .await?
            }
            _ => {
                let client = openai::CompletionsClient::from_env()?;
                let perm = permission::interactive();
                let mut builder = client
                    .agent(current_model.as_str())
                    .preamble(&system_prompt)
                    .tools(all_tools(dir, perm, &current_model, provider, thinking))
                    .max_tokens(8192)
                    .default_max_turns(20);
                if thinking {
                    builder =
                        builder.additional_params(serde_json::json!({ "enable_thinking": true }));
                }
                let agent = builder.build();
                run_loop(
                    agent,
                    client,
                    dir,
                    &current_model,
                    provider,
                    thinking,
                    context_size,
                    &mut session,
                    &mut history,
                    &mut scratchpad,
                    first_start,
                )
                .instrument(session_span.clone())
                .await?
            }
        };

        first_start = false;

        match new_model {
            None => break,
            Some(m) => {
                let mut settings = Settings::load();
                if let Err(e) = settings.set_model(&m) {
                    tracing::warn!(error = %e, "failed to save model to settings");
                }
                current_model = m;
            }
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn run_loop<M, C>(
    agent: Agent<M>,
    client: C,
    dir: &str,
    model: &str,
    provider: &str,
    thinking: bool,
    context_size: u64,
    session: &mut Session,
    history: &mut Vec<Message>,
    scratchpad: &mut String,
    first_start: bool,
) -> Result<Option<String>>
where
    M: CompletionModel + 'static,
    Agent<M>: StreamingChat<M, M::StreamingResponse>,
    C: CompletionClient + Clone + Send + Sync + 'static,
    C::CompletionModel: CompletionModel + 'static,
{
    let session_path = session.path.display().to_string();
    if first_start {
        ui::print_banner(model, dir, &session_path, thinking);
    } else {
        ui::print_model_switch(model, dir, &session_path, thinking);
    }

    let mut recorder = TrajectoryRecorder::new(&session.path)?;

    let mut ctx = ContextStats::new();
    let mut rl = DefaultEditor::new()?;

    loop {
        let readline_prompt = "\x01\x1b[32m\x02❯ \x01\x1b[0m\x02";
        match rl.readline(readline_prompt) {
            Ok(line) => {
                let line = line.trim().to_string();
                if line.is_empty() {
                    continue;
                }
                if line == "exit" || line == "quit" {
                    return Ok(None);
                }
                if line == "/model" {
                    if let Some(new_model) = handle_model_command(provider, model, &mut rl).await? {
                        ui::print_model_changed(&new_model);
                        return Ok(Some(new_model));
                    }
                    continue;
                }
                let _ = rl.add_history_entry(&line);

                session.log_user(&line)?;
                recorder.start_turn(&line);
                ui::print_working();

                let turn_span = tracing::info_span!(
                    "turn",
                    history_len = history.len(),
                    input_tokens = ctx.input_tokens,
                );

                // Prepend the scratchpad as a pseudo-message so the model
                // retains summarised context even after compaction.
                // fin.history() returns only NEW messages so history stays clean.
                let effective_history = build_effective_history(history, scratchpad);
                let mut stream = agent.stream_chat(line.as_str(), &effective_history).await;

                let turn_result = tokio::select! {
                    biased;
                    result = drive_stream(&mut stream, thinking, false, history, &mut ctx, &mut recorder)
                        .instrument(turn_span) => Some(result),
                    _ = tokio::time::sleep(TURN_TIMEOUT) => {
                        ui::print_error("turn timed out after 5 minutes");
                        None
                    }
                    _ = tokio::signal::ctrl_c() => {
                        ui::print_cancelled();
                        None
                    }
                };

                if let Some(turn_result) = turn_result {
                    match turn_result {
                        Ok((text, last_usage)) => {
                            session.log_agent(&text)?;

                            // 1. Compress old tool results (cheap, no model call).
                            context::compress_tool_results(history);

                            // 2. Summarise + compact if context is filling up.
                            let compacted =
                                if context::needs_compaction(last_usage.input_tokens, context_size)
                                {
                                    tracing::info!(
                                        input_tokens = last_usage.input_tokens,
                                        context_size,
                                        history_len = history.len(),
                                        "compacting context"
                                    );
                                    ui::print_compacting();
                                    let did_compact = context::compact_with_summary(
                                        history, scratchpad, &client, model,
                                    )
                                    .await
                                    .unwrap_or(false);
                                    if did_compact {
                                        tracing::info!(
                                            history_len = history.len(),
                                            "compaction done"
                                        );
                                    }
                                    did_compact
                                } else {
                                    false
                                };

                            // 3. Persist conversation state so a crash loses at most one turn.
                            if let Err(e) = session.save_state(history, scratchpad) {
                                tracing::warn!(error = %e, "failed to save session state");
                            }

                            ui::print_context_stats(
                                ctx.input_tokens,
                                ctx.output_tokens,
                                ctx.total_tokens,
                                ctx.cached_input_tokens,
                                ctx.reasoning_tokens,
                                last_usage.input_tokens,
                                context_size,
                                compacted,
                            );
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "turn failed");
                            ui::print_error(&e.to_string());
                        }
                    }
                }
            }
            Err(ReadlineError::Eof | ReadlineError::Interrupted) => return Ok(None),
            Err(e) => return Err(e.into()),
        }
    }
}

/// Streams reasoning text to the terminal in real time with a `│ ` border on
/// every line. Chunks are printed immediately — no buffering for whole lines.
struct ThinkingPrinter {
    started: bool,
    at_line_start: bool,
}

impl ThinkingPrinter {
    fn new() -> Self {
        Self {
            started: false,
            at_line_start: true,
        }
    }

    fn feed(&mut self, chunk: &str) {
        if chunk.is_empty() {
            return;
        }
        if !self.started {
            ui::stream_thinking_start();
            self.started = true;
        }
        // Split on newlines; print the prefix at the start of each line.
        let mut parts = chunk.split('\n');
        if let Some(first) = parts.next() {
            if !first.is_empty() {
                if self.at_line_start {
                    ui::stream_thinking_prefix();
                    self.at_line_start = false;
                }
                ui::stream_thinking_chunk(first);
            }
            for part in parts {
                // '\n' before each subsequent part
                ui::stream_thinking_chunk("\n");
                self.at_line_start = true;
                if !part.is_empty() {
                    ui::stream_thinking_prefix();
                    self.at_line_start = false;
                    ui::stream_thinking_chunk(part);
                }
            }
        }
        if chunk.ends_with('\n') {
            self.at_line_start = true;
        }
    }

    fn finish(&mut self) {
        if self.started {
            ui::stream_thinking_end();
            self.started = false;
            self.at_line_start = true;
        }
    }
}

const TOOL_CALL_TAG: &str = "<tool_call>";
const THINK_OPEN: &str = "<think>";
const THINK_CLOSE: &str = "</think>";

/// Return the length of the longest suffix of `text` that is a prefix of `needle`.
fn suffix_prefix_len(text: &str, needle: &str) -> usize {
    for len in (1..=needle.len().min(text.len())).rev() {
        if text.ends_with(&needle[..len]) {
            return len;
        }
    }
    0
}

/// Find the byte position of the first `<name>` where `name` is at least two
/// chars of `[a-z0-9_]` (snake_case tool tag). Returns `(pos, tag_end)` where
/// `tag_end` is the index just after the closing `>`.
fn find_tool_open(text: &str) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'<' {
            let name_start = i + 1;
            let mut j = name_start;
            while j < bytes.len()
                && (bytes[j].is_ascii_lowercase() || bytes[j] == b'_' || bytes[j].is_ascii_digit())
            {
                j += 1;
            }
            let name_len = j - name_start;
            if name_len >= 2 && j < bytes.len() && bytes[j] == b'>' {
                return Some((i, j + 1));
            }
        }
        i += 1;
    }
    None
}

/// Length of a trailing partial tool open tag like `<to`, `<todo_write`, or `<`.
fn trailing_tool_tag_len(text: &str) -> usize {
    let bytes = text.as_bytes();
    let n = bytes.len();
    if n == 0 {
        return 0;
    }
    // Walk backward over valid tag-name chars until we hit `<` or something else.
    let mut i = n;
    loop {
        if i == 0 {
            return 0;
        }
        i -= 1;
        if bytes[i] == b'<' {
            return n - i;
        }
        if !bytes[i].is_ascii_lowercase() && bytes[i] != b'_' && !bytes[i].is_ascii_digit() {
            return 0;
        }
    }
}

/// In normal streaming mode, find the first interesting tag (`<think>` or any
/// snake_case tool tag like `<tool_call>`, `<todo_write>`, etc.).
/// Returns `(safe_to_display, rest_after_tag, which_tag)` or `(safe, tail, "")`
/// if no complete tag is present yet.
fn scan_normal(text: &str) -> (&str, &str, &str) {
    let think_pos = text.find(THINK_OPEN);
    let tool_open = find_tool_open(text);

    match (think_pos, tool_open) {
        (Some(t), Some((tc, tc_end))) => {
            if t <= tc {
                (&text[..t], &text[t + THINK_OPEN.len()..], THINK_OPEN)
            } else {
                (&text[..tc], &text[tc_end..], TOOL_CALL_TAG)
            }
        }
        (Some(t), None) => (&text[..t], &text[t + THINK_OPEN.len()..], THINK_OPEN),
        (None, Some((tc, tc_end))) => (&text[..tc], &text[tc_end..], TOOL_CALL_TAG),
        (None, None) => {
            // No complete tag; hold back any potential partial tag at the end.
            let hold = suffix_prefix_len(text, THINK_OPEN).max(trailing_tool_tag_len(text));
            (&text[..text.len() - hold], &text[text.len() - hold..], "")
        }
    }
}

enum StreamState {
    /// Streaming normal text; `tail` is held back while we confirm it is not
    /// the start of `<think>` or `<tool_call>`.
    Streaming { tail: String },
    /// Inside `<think>…</think>` — route text to `ThinkingPrinter`.
    /// `tail` guards against a partial `</think>` at the end of a chunk.
    InThink { tail: String },
    /// A structured ToolCall/ToolCallDelta arrived — discard text until the
    /// next tool result, then reset to `Streaming`.
    InToolCall,
}

/// Consume the streaming response, printing text/thinking chunks to the
/// terminal as they arrive. Updates `history` and `ctx` from the `FinalResponse`.
/// Returns `(response_text, last_turn_usage)` for session logging and compaction.
async fn drive_stream<R>(
    stream: &mut rig::agent::StreamingResult<R>,
    thinking: bool,
    silent: bool,
    history: &mut Vec<Message>,
    ctx: &mut ContextStats,
    recorder: &mut TrajectoryRecorder,
) -> Result<(String, Usage)> {
    let mut state = StreamState::Streaming {
        tail: String::new(),
    };
    let mut response_text = String::new();
    let mut response_started = false;
    let mut thinker = ThinkingPrinter::new();
    let mut last_usage = Usage::new();

    while let Some(item) = stream.next().await {
        match item {
            Ok(MultiTurnStreamItem::StreamAssistantItem(content)) => {
                match content {
                    StreamedAssistantContent::Text(text) => {
                        // Process the incoming chunk according to the current state.
                        // We loop to handle a state transition mid-chunk (e.g. the
                        // chunk contains both "</think>" and subsequent normal text).
                        let mut remaining: String = text.text.clone();
                        loop {
                            if remaining.is_empty() {
                                break;
                            }
                            match state {
                                StreamState::Streaming { ref mut tail } => {
                                    let mut combined = std::mem::take(tail);
                                    combined.push_str(&remaining);
                                    remaining = String::new();

                                    let (safe, rest, tag) = scan_normal(&combined);
                                    if !safe.is_empty() {
                                        emit_text(
                                            safe,
                                            &mut response_started,
                                            &mut response_text,
                                            silent,
                                        );
                                    }
                                    match tag {
                                        THINK_OPEN => {
                                            state = StreamState::InThink {
                                                tail: String::new(),
                                            };
                                            remaining = rest.to_string();
                                        }
                                        TOOL_CALL_TAG => {
                                            state = StreamState::InToolCall;
                                            // rest is after the tool call tag — discard for now
                                        }
                                        _ => {
                                            // no tag found; rest is the look-ahead tail
                                            *tail = rest.to_string();
                                        }
                                    }
                                }
                                StreamState::InThink { ref mut tail } => {
                                    let mut combined = std::mem::take(tail);
                                    combined.push_str(&remaining);
                                    remaining = String::new();

                                    if let Some(pos) = combined.find(THINK_CLOSE) {
                                        // Feed content before </think> to thinker, close it.
                                        if thinking {
                                            thinker.feed(&combined[..pos]);
                                        }
                                        thinker.finish();
                                        let after = combined[pos + THINK_CLOSE.len()..].to_string();
                                        state = StreamState::Streaming {
                                            tail: String::new(),
                                        };
                                        remaining = after; // process remainder as normal
                                    } else {
                                        // Hold back potential partial </think> at end.
                                        let hold = suffix_prefix_len(&combined, THINK_CLOSE);
                                        if thinking {
                                            thinker.feed(&combined[..combined.len() - hold]);
                                        }
                                        *tail = combined[combined.len() - hold..].to_string();
                                    }
                                }
                                StreamState::InToolCall => break, // discard
                            }
                        }
                    }
                    StreamedAssistantContent::ToolCallDelta { .. } => {
                        // Structured tool-call event — the tail was markup, drop it.
                        state = StreamState::InToolCall;
                    }
                    StreamedAssistantContent::ToolCall { tool_call, .. } => {
                        state = StreamState::InToolCall;
                        recorder.tool_call(
                            &tool_call.id,
                            &tool_call.function.name,
                            Some(tool_call.function.arguments.clone()),
                        );
                    }
                    StreamedAssistantContent::ReasoningDelta { reasoning, .. } => {
                        recorder.feed_thinking(&reasoning);
                        if thinking {
                            thinker.feed(&reasoning);
                        }
                    }
                    StreamedAssistantContent::Reasoning(r) => {
                        let text = r.display_text();
                        recorder.feed_thinking(&text);
                        recorder.finish_thinking();
                        if thinking {
                            thinker.feed(&text);
                            thinker.finish();
                        }
                    }
                    _ => {}
                }
            }
            Ok(MultiTurnStreamItem::StreamUserItem(item)) => {
                // Tool result consumed — record it and reset streaming.
                let rig::streaming::StreamedUserContent::ToolResult { tool_result, .. } = &item;
                let output = tool_result
                    .content
                    .iter()
                    .filter_map(|c| {
                        if let rig::completion::message::ToolResultContent::Text(t) = c {
                            Some(t.text.as_str())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                recorder.tool_result(&tool_result.id, &output);
                state = StreamState::Streaming {
                    tail: String::new(),
                };
            }
            Ok(MultiTurnStreamItem::FinalResponse(fin)) => {
                // Flush any pending look-ahead tails.
                match &state {
                    StreamState::Streaming { tail } if !tail.is_empty() => {
                        emit_text(tail, &mut response_started, &mut response_text, silent);
                    }
                    StreamState::InThink { tail } => {
                        if thinking && !tail.is_empty() {
                            recorder.feed_thinking(tail);
                            if thinking {
                                thinker.feed(tail);
                            }
                        }
                        recorder.finish_thinking();
                        thinker.finish();
                    }
                    _ => {}
                }
                last_usage = handle_final(&fin, &mut response_text, history);
                ctx.add(&last_usage);
                if let Err(e) = recorder.finish_turn(
                    &response_text,
                    last_usage.input_tokens,
                    last_usage.output_tokens,
                    last_usage.cached_input_tokens,
                    last_usage.reasoning_tokens,
                ) {
                    tracing::warn!(error = %e, "failed to write trajectory record");
                }
                break;
            }
            Ok(_) => {}
            Err(e) => {
                tracing::error!(error = %e, "stream error");
                thinker.finish();
                if response_started {
                    ui::stream_response_end();
                }
                return Err(anyhow::anyhow!("{e}"));
            }
        }
    }

    thinker.finish();
    if response_started && !silent {
        ui::stream_response_end();
    }
    Ok((response_text, last_usage))
}

/// Write `chunk` to the terminal and append it to `response_text`.
/// When `silent` is true the chunk is still accumulated but not printed.
fn emit_text(chunk: &str, response_started: &mut bool, response_text: &mut String, silent: bool) {
    if !silent {
        if !*response_started {
            ui::stream_response_start();
            *response_started = true;
        }
        ui::stream_text(chunk);
    }
    response_text.push_str(chunk);
}

/// Prepend a scratchpad message to history when one exists.
/// This keeps `history` clean (only real messages) while still giving the
/// model access to summarised context from previous compaction cycles.
fn build_effective_history(history: &[Message], scratchpad: &str) -> Vec<Message> {
    if scratchpad.is_empty() {
        return history.to_vec();
    }
    let mut effective = Vec::with_capacity(history.len() + 1);
    effective.push(Message::user(format!("[Session Memory]\n{scratchpad}")));
    effective.extend_from_slice(history);
    effective
}

fn handle_final(
    fin: &FinalResponse,
    response_text: &mut String,
    history: &mut Vec<Message>,
) -> Usage {
    if response_text.is_empty() && !fin.response().is_empty() {
        response_text.push_str(fin.response());
        ui::print_response(fin.response());
    }
    if let Some(new_msgs) = fin.history() {
        history.extend_from_slice(new_msgs);
    }
    fin.usage()
}

/// Run a single task headlessly: build an agent, stream the response to
/// completion, and return the final text. Used by the `SpawnAgent` tool.
///
/// The subagent gets only the base tools (no recursive spawn), and all tool
/// permission prompts are auto-approved so the call is non-interactive.
pub async fn run_task(
    dir: &str,
    model: &str,
    provider: &str,
    task: &str,
    thinking: bool,
    trajectory_path: Option<&std::path::Path>,
) -> anyhow::Result<String> {
    let system_prompt = build_system_prompt(dir);
    let perm = crate::permission::auto_approve();

    let recorder_path = match trajectory_path {
        Some(p) => p.to_path_buf(),
        None => {
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            std::env::temp_dir().join(format!("kapitoshka_sub_{ts}.md"))
        }
    };

    match provider {
        "anthropic" => {
            let client = anthropic::Client::from_env()?;
            let agent = client
                .agent(model)
                .preamble(&system_prompt)
                .tools(crate::tools::base_tools(dir, perm))
                .max_tokens(8192)
                .default_max_turns(20)
                .build();
            execute_task(agent, task, thinking, &recorder_path).await
        }
        _ => {
            let client = openai::CompletionsClient::from_env()?;
            let mut builder = client
                .agent(model)
                .preamble(&system_prompt)
                .tools(crate::tools::base_tools(dir, perm))
                .max_tokens(8192)
                .default_max_turns(20);
            if thinking {
                builder = builder.additional_params(serde_json::json!({ "enable_thinking": true }));
            }
            let agent = builder.build();
            execute_task(agent, task, thinking, &recorder_path).await
        }
    }
}

/// Internal generic driver for `run_task`. Runs the agent stream to completion
/// without any interactive I/O.
async fn execute_task<M>(
    agent: rig::agent::Agent<M>,
    task: &str,
    thinking: bool,
    recorder_path: &std::path::Path,
) -> anyhow::Result<String>
where
    M: CompletionModel + 'static,
    rig::agent::Agent<M>: StreamingChat<M, M::StreamingResponse>,
{
    let mut recorder = TrajectoryRecorder::new(recorder_path)?;
    recorder.start_turn(task);
    let mut history = Vec::new();
    let mut ctx = ContextStats::new();
    let mut stream = agent.stream_chat(task, &[] as &[Message]).await;
    let (text, _) = drive_stream(
        &mut stream,
        thinking,
        true,
        &mut history,
        &mut ctx,
        &mut recorder,
    )
    .await?;
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rig::completion::Message;

    // ── suffix_prefix_len ────────────────────────────────────────────────────

    #[test]
    fn suffix_prefix_no_overlap() {
        assert_eq!(suffix_prefix_len("hello world", "<think>"), 0);
    }

    #[test]
    fn suffix_prefix_partial_think() {
        assert_eq!(suffix_prefix_len("foo <thi", "<think>"), 4);
    }

    #[test]
    fn suffix_prefix_full_needle_length() {
        // When text ends with the complete needle, the full needle length is returned.
        // scan_normal's find() catches complete tags first, so this only guards edge cases.
        assert_eq!(
            suffix_prefix_len("foo <think>", "<think>"),
            THINK_OPEN.len()
        );
    }

    #[test]
    fn suffix_prefix_single_char_overlap() {
        assert_eq!(suffix_prefix_len("text <", "<think>"), 1);
    }

    #[test]
    fn suffix_prefix_tool_call_partial() {
        assert_eq!(suffix_prefix_len("end <tool", TOOL_CALL_TAG), 5);
    }

    // ── scan_normal ──────────────────────────────────────────────────────────

    #[test]
    fn scan_normal_no_tags() {
        let (safe, tail, tag) = scan_normal("hello world");
        assert_eq!(safe, "hello world");
        assert_eq!(tag, "");
        assert_eq!(tail, "");
    }

    #[test]
    fn scan_normal_holds_partial_think() {
        let (safe, tail, tag) = scan_normal("hello <thi");
        assert_eq!(safe, "hello ");
        assert_eq!(tag, "");
        assert_eq!(tail, "<thi");
    }

    #[test]
    fn scan_normal_complete_think_tag() {
        let (safe, rest, tag) = scan_normal("before <think> after");
        assert_eq!(safe, "before ");
        assert_eq!(tag, THINK_OPEN);
        assert_eq!(rest, " after");
    }

    #[test]
    fn scan_normal_complete_tool_call_tag() {
        let (safe, rest, tag) = scan_normal("before <tool_call> after");
        assert_eq!(safe, "before ");
        assert_eq!(tag, TOOL_CALL_TAG);
        assert_eq!(rest, " after");
    }

    #[test]
    fn scan_normal_think_before_tool() {
        let (safe, rest, tag) = scan_normal("a <think>b<tool_call>c");
        assert_eq!(safe, "a ");
        assert_eq!(tag, THINK_OPEN);
        assert_eq!(rest, "b<tool_call>c");
    }

    #[test]
    fn scan_normal_tool_before_think() {
        let (safe, rest, tag) = scan_normal("a <tool_call>b<think>c");
        assert_eq!(safe, "a ");
        assert_eq!(tag, TOOL_CALL_TAG);
        assert_eq!(rest, "b<think>c");
    }

    #[test]
    fn scan_normal_suppresses_named_tool_tag() {
        let (safe, rest, tag) = scan_normal("before <todo_write>{\"todos\":[]}");
        assert_eq!(safe, "before ");
        assert_eq!(tag, TOOL_CALL_TAG);
        assert_eq!(rest, "{\"todos\":[]}");
    }

    #[test]
    fn scan_normal_holds_partial_tool_tag() {
        let (safe, tail, tag) = scan_normal("text <todo");
        assert_eq!(safe, "text ");
        assert_eq!(tag, "");
        assert_eq!(tail, "<todo");
    }

    // ── ContextStats ─────────────────────────────────────────────────────────

    fn make_usage(input: u64, output: u64, total: u64) -> Usage {
        Usage {
            input_tokens: input,
            output_tokens: output,
            total_tokens: total,
            cached_input_tokens: 0,
            cache_creation_input_tokens: 0,
            reasoning_tokens: 0,
        }
    }

    #[test]
    fn context_stats_accumulates() {
        let mut ctx = ContextStats::new();
        ctx.add(&make_usage(100, 50, 150));
        ctx.add(&make_usage(200, 80, 280));
        assert_eq!(ctx.input_tokens, 300);
        assert_eq!(ctx.output_tokens, 130);
        assert_eq!(ctx.total_tokens, 430);
    }

    #[test]
    fn context_stats_starts_zero() {
        let ctx = ContextStats::new();
        assert_eq!(ctx.input_tokens, 0);
        assert_eq!(ctx.output_tokens, 0);
        assert_eq!(ctx.total_tokens, 0);
    }

    // ── build_effective_history ──────────────────────────────────────────────

    #[test]
    fn effective_history_no_scratchpad_is_clone() {
        let history = vec![Message::user("hello"), Message::user("world")];
        let eff = build_effective_history(&history, "");
        assert_eq!(eff.len(), history.len());
    }

    #[test]
    fn effective_history_prepends_scratchpad_message() {
        let history = vec![Message::user("task")];
        let eff = build_effective_history(&history, "- prior fact");
        assert_eq!(eff.len(), 2);
        match &eff[0] {
            Message::User { content } => {
                let first = content.first();
                match first {
                    rig::completion::message::UserContent::Text(t) => {
                        assert!(t.text.contains("[Session Memory]"));
                        assert!(t.text.contains("prior fact"));
                    }
                    _ => panic!("expected text"),
                }
            }
            _ => panic!("expected User message"),
        }
        // Original history follows unchanged.
        match &eff[1] {
            Message::User { content } => {
                let first = content.first();
                match first {
                    rig::completion::message::UserContent::Text(t) => {
                        assert_eq!(t.text, "task");
                    }
                    _ => panic!("expected text"),
                }
            }
            _ => panic!("expected User message"),
        }
    }

    #[test]
    fn effective_history_does_not_mutate_original() {
        let history = vec![Message::user("a"), Message::user("b")];
        let _ = build_effective_history(&history, "memo");
        assert_eq!(history.len(), 2);
    }

    // ── extract_agent_rules ──────────────────────────────────────────────────

    #[test]
    fn agent_rules_global_only() {
        let md = "Use conventional commits.\nRun tests before pushing.\n";
        let rules = extract_agent_rules(md, "kapitoshka");
        assert!(rules.contains("Use conventional commits"));
        assert!(rules.contains("Run tests"));
    }

    #[test]
    fn agent_rules_picks_matching_section() {
        let md = "\
Global rule.

## kapitoshka
Agent-specific rule.

## other-agent
Should not appear.
";
        let rules = extract_agent_rules(md, "kapitoshka");
        assert!(rules.contains("Global rule"));
        assert!(rules.contains("Agent-specific rule"));
        assert!(!rules.contains("Should not appear"));
    }

    #[test]
    fn agent_rules_skips_non_matching_section() {
        let md = "\
## other-agent
Other rule.
";
        let rules = extract_agent_rules(md, "kapitoshka");
        assert!(rules.is_empty());
    }

    #[test]
    fn agent_rules_case_insensitive_heading() {
        let md = "\
## Kapitoshka
My rule.
";
        let rules = extract_agent_rules(md, "kapitoshka");
        assert!(rules.contains("My rule"));
    }

    #[test]
    fn agent_rules_no_agent_sections() {
        let md = "Just global rules here.";
        let rules = extract_agent_rules(md, "kapitoshka");
        assert_eq!(rules, "Just global rules here.");
    }
}
