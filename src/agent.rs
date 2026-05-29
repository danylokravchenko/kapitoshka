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

    let agent = client
        .agent(model)
        .preamble(SYSTEM_PROMPT)
        .tools(all_tools(dir, perm))
        .max_tokens(8192)
        .default_max_turns(20)
        .build();

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

/// The tool-call tag emitted by local models as plain text before it is
/// converted into a structured event.
const TOOL_CALL_TAG: &str = "<tool_call>";

/// Split `text` into the portion safe to display immediately and a look-ahead
/// tail that must be held back until we can confirm it is not the start of
/// `<tool_call>`.
///
/// Returns `(safe, tail)`:
/// - `safe` — everything before any `<tool_call>` (or before a possible prefix)
/// - `tail` — at most `TOOL_CALL_TAG.len()` chars held back
///
/// If the full tag is present in `text`, `tail` is empty and the caller should
/// transition to `InToolCall`.
fn split_safe(text: &str) -> (&str, &str) {
    // Fast path: no `<` at all → nothing to hold back.
    if !text.contains('<') {
        return (text, "");
    }
    // Full tag present → safe is everything before it, tail is empty.
    if let Some(pos) = text.find(TOOL_CALL_TAG) {
        return (&text[..pos], "");
    }
    // Check whether the suffix of `text` is a prefix of the tag.
    let tag = TOOL_CALL_TAG.as_bytes();
    for hold in (1..tag.len().min(text.len()) + 1).rev() {
        let suffix = &text[text.len() - hold..];
        if TOOL_CALL_TAG.starts_with(suffix) {
            return (&text[..text.len() - hold], &text[text.len() - hold..]);
        }
    }
    (text, "")
}

enum StreamState {
    /// Streaming text to the terminal; `tail` is a short look-ahead buffer
    /// held back while we decide if it is the start of `<tool_call>`.
    Streaming { tail: String },
    /// A ToolCall/ToolCallDelta arrived — discard everything until the next
    /// tool result, then reset to `Streaming`.
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
    let mut thinking_started = false;

    while let Some(item) = stream.next().await {
        match item {
            Ok(MultiTurnStreamItem::StreamAssistantItem(content)) => {
                match content {
                    StreamedAssistantContent::Text(text) => {
                        if thinking_started {
                            ui::stream_thinking_end();
                            thinking_started = false;
                        }
                        if let StreamState::Streaming { ref mut tail } = state {
                            // Prepend look-ahead tail to the new chunk.
                            let mut combined = std::mem::take(tail);
                            combined.push_str(&text.text);

                            let (safe, new_tail) = split_safe(&combined);

                            // Check if the full tag was detected.
                            if combined.contains(TOOL_CALL_TAG) {
                                // Flush whatever came before the tag, then suppress.
                                if !safe.is_empty() {
                                    emit_text(
                                        safe,
                                        &mut response_started,
                                        &mut response_text,
                                    );
                                }
                                state = StreamState::InToolCall;
                            } else {
                                if !safe.is_empty() {
                                    emit_text(
                                        safe,
                                        &mut response_started,
                                        &mut response_text,
                                    );
                                }
                                *tail = new_tail.to_string();
                            }
                        }
                        // In InToolCall: discard all text (still markup).
                    }
                    StreamedAssistantContent::ToolCallDelta { .. }
                    | StreamedAssistantContent::ToolCall { .. } => {
                        // Structured tool-call event — the tail was markup, drop it.
                        state = StreamState::InToolCall;
                    }
                    StreamedAssistantContent::ReasoningDelta { reasoning, .. } if thinking => {
                        if !thinking_started {
                            ui::stream_thinking_start();
                            thinking_started = true;
                        }
                        ui::stream_thinking(&reasoning);
                    }
                    StreamedAssistantContent::Reasoning(r) if thinking => {
                        if !thinking_started {
                            ui::stream_thinking_start();
                        }
                        ui::stream_thinking(&r.display_text());
                        ui::stream_thinking_end();
                        thinking_started = false;
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
                // Flush the look-ahead tail (no full tag arrived, so it is safe).
                if let StreamState::Streaming { ref tail } = state {
                    if !tail.is_empty() {
                        emit_text(tail, &mut response_started, &mut response_text);
                    }
                }
                handle_final(&fin, &mut response_text, history);
                break;
            }
            Ok(_) => {}
            Err(e) => {
                end_visual_state(response_started, thinking_started);
                return Err(anyhow::anyhow!("{e}"));
            }
        }
    }

    end_visual_state(response_started, thinking_started);
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

fn end_visual_state(response_started: bool, thinking_started: bool) {
    if response_started {
        ui::stream_response_end();
    } else if thinking_started {
        ui::stream_thinking_end();
    }
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
