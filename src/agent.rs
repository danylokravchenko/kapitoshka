use anyhow::Result;
use futures::StreamExt;
use rig::agent::{FinalResponse, MultiTurnStreamItem};
use rig::client::{CompletionClient, ProviderClient};
use rig::completion::{Message, Usage};
use rig::providers::openai::CompletionsClient;
use rig::streaming::{StreamedAssistantContent, StreamingChat};
use rustyline::DefaultEditor;
use rustyline::error::ReadlineError;

use crate::context;
use crate::permission;
use crate::session::Session;
use crate::tools::all_tools;
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

const SYSTEM_PROMPT: &str = "\
You are kapitoshka, an expert coding agent. You help users with software engineering tasks.

You have access to tools to read files, write files, list directories, and run shell commands.
Always explore the codebase before making changes. Prefer targeted edits over full rewrites.
After making changes, verify them by reading the modified files or running relevant commands.

Working directory is provided in the task. Use it as the root for all file operations.";

pub async fn run_interactive(
    dir: &str,
    model: &str,
    thinking: bool,
    context_size: u64,
) -> Result<()> {
    let client = CompletionsClient::from_env()?;
    let perm = permission::interactive();

    let mut builder = client
        .agent(model)
        .preamble(SYSTEM_PROMPT)
        .tools(all_tools(dir, perm))
        .max_tokens(8192)
        .default_max_turns(20);

    if thinking {
        // Pass enable_thinking to the inference server via the flattened
        // additional_params field (supported by vLLM, some Ollama builds, etc.).
        builder = builder.additional_params(serde_json::json!({ "enable_thinking": true }));
    }

    let agent = builder.build();

    let mut session = Session::new(dir, model)?;
    let session_path = session.path.display().to_string();

    ui::print_banner(model, dir, &session_path, thinking);

    // Conversation history managed manually so we can append new messages
    // from each turn's FinalResponse.
    let mut history: Vec<Message> = Vec::new();
    // Scratchpad accumulates summaries from compaction and is prepended to
    // every request so the model retains context across compaction boundaries.
    let mut scratchpad = String::new();
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
                    break;
                }
                let _ = rl.add_history_entry(&line);

                session.log_user(&line)?;
                ui::print_working();

                // Prepend the scratchpad as a pseudo-message so the model
                // retains summarised context even after compaction.
                // fin.history() returns only NEW messages so history stays clean.
                let effective_history = build_effective_history(&history, &scratchpad);
                let mut stream = agent.stream_chat(line.as_str(), &effective_history).await;

                match drive_stream(&mut stream, thinking, &mut history, &mut ctx).await {
                    Ok((text, last_usage)) => {
                        session.log_agent(&text)?;

                        // 1. Compress old tool results (cheap, no model call).
                        context::compress_tool_results(&mut history);

                        // 2. Summarise + compact if context is filling up.
                        let compacted =
                            if context::needs_compaction(last_usage.input_tokens, context_size) {
                                ui::print_compacting();
                                context::compact_with_summary(
                                    &mut history,
                                    &mut scratchpad,
                                    &client,
                                    model,
                                )
                                .await
                                .unwrap_or(false)
                            } else {
                                false
                            };

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
                    Err(e) => ui::print_error(&e.to_string()),
                }
            }
            Err(ReadlineError::Eof | ReadlineError::Interrupted) => break,
            Err(e) => return Err(e.into()),
        }
    }

    Ok(())
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

/// In normal streaming mode, find the first interesting tag (`<think>` or
/// `<tool_call>`). Returns `(safe_to_display, tag_start, which_tag)` or
/// `(safe, tail, "")` if no complete tag is present.
fn scan_normal(text: &str) -> (&str, &str, &str) {
    let think_pos = text.find(THINK_OPEN);
    let tool_pos = text.find(TOOL_CALL_TAG);

    match (think_pos, tool_pos) {
        (Some(t), Some(tc)) => {
            let first = t.min(tc);
            let tag = if t <= tc { THINK_OPEN } else { TOOL_CALL_TAG };
            (&text[..first], &text[first + tag.len()..], tag)
        }
        (Some(t), None) => (&text[..t], &text[t + THINK_OPEN.len()..], THINK_OPEN),
        (None, Some(tc)) => (
            &text[..tc],
            &text[tc + TOOL_CALL_TAG.len()..],
            TOOL_CALL_TAG,
        ),
        (None, None) => {
            // No complete tag; hold back any potential partial tag at the end.
            let hold =
                suffix_prefix_len(text, THINK_OPEN).max(suffix_prefix_len(text, TOOL_CALL_TAG));
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
async fn drive_stream(
    stream: &mut rig::agent::StreamingResult<
        <rig::providers::openai::GenericCompletionModel as rig::completion::CompletionModel>::StreamingResponse,
    >,
    thinking: bool,
    history: &mut Vec<Message>,
    ctx: &mut ContextStats,
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
                                        emit_text(safe, &mut response_started, &mut response_text);
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
                    StreamedAssistantContent::ToolCallDelta { .. }
                    | StreamedAssistantContent::ToolCall { .. } => {
                        // Structured tool-call event — the tail was markup, drop it.
                        state = StreamState::InToolCall;
                    }
                    StreamedAssistantContent::ReasoningDelta { reasoning, .. } if thinking => {
                        thinker.feed(&reasoning);
                    }
                    StreamedAssistantContent::Reasoning(r) if thinking => {
                        thinker.feed(&r.display_text());
                        thinker.finish();
                    }
                    _ => {}
                }
            }
            Ok(MultiTurnStreamItem::StreamUserItem(_)) => {
                // Tool result consumed — model will now produce more text (or
                // call another tool). Reset streaming with a fresh look-ahead.
                state = StreamState::Streaming {
                    tail: String::new(),
                };
            }
            Ok(MultiTurnStreamItem::FinalResponse(fin)) => {
                // Flush any pending look-ahead tails.
                match &state {
                    StreamState::Streaming { tail } if !tail.is_empty() => {
                        emit_text(tail, &mut response_started, &mut response_text);
                    }
                    StreamState::InThink { tail } => {
                        if thinking && !tail.is_empty() {
                            thinker.feed(tail);
                        }
                        thinker.finish();
                    }
                    _ => {}
                }
                last_usage = handle_final(&fin, &mut response_text, history);
                ctx.add(&last_usage);
                break;
            }
            Ok(_) => {}
            Err(e) => {
                thinker.finish();
                if response_started {
                    ui::stream_response_end();
                }
                return Err(anyhow::anyhow!("{e}"));
            }
        }
    }

    thinker.finish();
    if response_started {
        ui::stream_response_end();
    }
    Ok((response_text, last_usage))
}

/// Write `chunk` to the terminal and append it to `response_text`.
fn emit_text(chunk: &str, response_started: &mut bool, response_text: &mut String) {
    if !*response_started {
        ui::stream_response_start();
        *response_started = true;
    }
    ui::stream_text(chunk);
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
}
