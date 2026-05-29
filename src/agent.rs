use anyhow::Result;
use futures::StreamExt;
use rig::agent::{FinalResponse, MultiTurnStreamItem};
use rig::client::{CompletionClient, ProviderClient};
use rig::completion::Message;
use rig::providers::openai::CompletionsClient;
use rig::streaming::{StreamedAssistantContent, StreamingChat};
use rustyline::DefaultEditor;
use rustyline::error::ReadlineError;

use crate::permission;
use crate::session::Session;
use crate::tools::all_tools;
use crate::ui;

const SYSTEM_PROMPT: &str = "\
You are kapitoshka, an expert coding agent. You help users with software engineering tasks.

You have access to tools to read files, write files, list directories, and run shell commands.
Always explore the codebase before making changes. Prefer targeted edits over full rewrites.
After making changes, verify them by reading the modified files or running relevant commands.

Working directory is provided in the task. Use it as the root for all file operations.";

pub async fn run_interactive(dir: &str, model: &str, thinking: bool) -> Result<()> {
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

                let mut stream = agent.stream_chat(line.as_str(), &history).await;

                let response_text = drive_stream(&mut stream, thinking, &mut history).await;

                match response_text {
                    Ok(text) => session.log_agent(&text)?,
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
        (None, Some(tc)) => (&text[..tc], &text[tc + TOOL_CALL_TAG.len()..], TOOL_CALL_TAG),
        (None, None) => {
            // No complete tag; hold back any potential partial tag at the end.
            let hold = suffix_prefix_len(text, THINK_OPEN)
                .max(suffix_prefix_len(text, TOOL_CALL_TAG));
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
/// terminal as they arrive. Updates `history` from the `FinalResponse`.
/// Returns the full response text for session logging.
async fn drive_stream(
    stream: &mut rig::agent::StreamingResult<
        <rig::providers::openai::GenericCompletionModel as rig::completion::CompletionModel>::StreamingResponse,
    >,
    thinking: bool,
    history: &mut Vec<Message>,
) -> Result<String> {
    let mut state = StreamState::Streaming { tail: String::new() };
    let mut response_text = String::new();
    let mut response_started = false;
    let mut thinker = ThinkingPrinter::new();

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
                                            state = StreamState::InThink { tail: String::new() };
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
                                        state = StreamState::Streaming { tail: String::new() };
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
                state = StreamState::Streaming { tail: String::new() };
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
                handle_final(&fin, &mut response_text, history);
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
    Ok(response_text)
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


fn handle_final(fin: &FinalResponse, response_text: &mut String, history: &mut Vec<Message>) {
    // FinalResponse::response() holds the concatenated text for the turn.
    // Use it only as a fallback if we haven't accumulated text via streaming.
    if response_text.is_empty() && !fin.response().is_empty() {
        response_text.push_str(fin.response());
        ui::print_response(fin.response());
    }
    if let Some(new_msgs) = fin.history() {
        history.extend_from_slice(new_msgs);
    }
}
