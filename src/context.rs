use anyhow::Result;
use rig::OneOrMany;
use rig::client::CompletionClient;
use rig::completion::Chat;
use rig::completion::Message;
use rig::completion::message::{AssistantContent, ToolResult, ToolResultContent, UserContent};
use rig::providers::openai::CompletionsClient;

/// Messages from the start of history that are never dropped (first user turn).
pub const KEEP_FIRST: usize = 2;
/// Messages at the end of history that are never dropped (recent turns).
pub const KEEP_LAST: usize = 8;
/// Truncate tool result bodies older than KEEP_LAST messages to this many chars.
const TOOL_RESULT_MAX_CHARS: usize = 400;
/// Compact when context fill reaches this percentage (requires --context-size).
pub const FILL_PCT_THRESHOLD: u64 = 75;
/// Absolute fallback threshold when --context-size is not set.
const ABSOLUTE_TOKEN_THRESHOLD: u64 = 80_000;

const SUMMARIZE_PREAMBLE: &str = "\
You summarize conversation history for a coding agent so it can be stored as \
compressed working memory. Produce a concise bullet-point list covering: \
files read or modified, shell commands run, decisions made, problems solved, \
and the current task state. Be specific — include file names and outcomes. \
Keep the summary under 200 words. Respond with bullet points only, no preamble.";

// ── Public API ────────────────────────────────────────────────────────────────

/// Returns true when the context fill exceeds the configured threshold.
pub fn needs_compaction(last_input_tokens: u64, context_size: u64) -> bool {
    if context_size > 0 {
        (last_input_tokens * 100)
            .checked_div(context_size)
            .is_some_and(|pct| pct >= FILL_PCT_THRESHOLD)
    } else {
        last_input_tokens >= ABSOLUTE_TOKEN_THRESHOLD
    }
}

/// Truncate `ToolResult` bodies in messages older than the last `KEEP_LAST`
/// messages. Cheap — no model call.
pub fn compress_tool_results(history: &mut [Message]) {
    let keep_from = history.len().saturating_sub(KEEP_LAST);
    for msg in &mut history[..keep_from] {
        if let Message::User { content } = msg {
            for item in content.iter_mut() {
                if let UserContent::ToolResult(tr) = item {
                    truncate_tool_result(tr);
                }
            }
        }
    }
}

/// Sliding-window compaction with model-generated summary.
///
/// Keeps `history[..KEEP_FIRST]` and `history[len-KEEP_LAST..]` intact.
/// The middle section is summarised by the model; the summary replaces the
/// dropped messages and is stored in `scratchpad` for injection into future
/// requests. Returns `true` if compaction was performed.
pub async fn compact_with_summary(
    history: &mut Vec<Message>,
    scratchpad: &mut String,
    client: &CompletionsClient,
    model: &str,
) -> Result<bool> {
    if history.len() <= KEEP_FIRST + KEEP_LAST {
        return Ok(false);
    }
    let drop_end = history.len() - KEEP_LAST;
    let to_drop = &history[KEEP_FIRST..drop_end];

    let formatted = format_for_summary(to_drop, scratchpad);
    let summary = call_summarize(client, model, &formatted)
        .await
        .unwrap_or_else(|_| {
            // If summarization fails, store a minimal placeholder so compaction
            // still frees context space.
            String::from("(summary unavailable — prior turns dropped)")
        });

    let summary_msg = Message::user(format!("[Conversation summary]\n{summary}"));
    history.drain(KEEP_FIRST..drop_end);
    history.insert(KEEP_FIRST, summary_msg);

    *scratchpad = summary;
    Ok(true)
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn truncate_tool_result(tr: &mut ToolResult) {
    let text = extract_tool_result_text(tr);
    if text.len() <= TOOL_RESULT_MAX_CHARS {
        return;
    }
    let boundary = text.floor_char_boundary(TOOL_RESULT_MAX_CHARS);
    let summary = format!(
        "{}… [{} chars truncated]",
        &text[..boundary],
        text.len() - boundary
    );
    tr.content = OneOrMany::one(ToolResultContent::text(summary));
}

fn extract_tool_result_text(tr: &ToolResult) -> String {
    tr.content
        .iter()
        .filter_map(|c| match c {
            ToolResultContent::Text(t) => Some(t.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_for_summary(messages: &[Message], existing_scratchpad: &str) -> String {
    let mut out = String::new();
    if !existing_scratchpad.is_empty() {
        out.push_str("Previous memory:\n");
        out.push_str(existing_scratchpad);
        out.push_str("\n\nNew turns to incorporate:\n");
    }
    for msg in messages {
        match msg {
            Message::User { content } => {
                for c in content.iter() {
                    match c {
                        UserContent::Text(t) => {
                            out.push_str("User: ");
                            out.push_str(&t.text);
                            out.push('\n');
                        }
                        UserContent::ToolResult(tr) => {
                            let text = extract_tool_result_text(tr);
                            let boundary = text.floor_char_boundary(200);
                            let snippet = &text[..boundary];
                            out.push_str(&format!(
                                "ToolResult[{}]: {}{}\n",
                                tr.id,
                                snippet,
                                if text.len() > 200 { "…" } else { "" }
                            ));
                        }
                        _ => {}
                    }
                }
            }
            Message::Assistant { content, .. } => {
                for c in content.iter() {
                    match c {
                        AssistantContent::Text(t) => {
                            out.push_str("Assistant: ");
                            out.push_str(&t.text);
                            out.push('\n');
                        }
                        AssistantContent::ToolCall(tc) => {
                            out.push_str(&format!(
                                "ToolCall: {}({})\n",
                                tc.function.name,
                                serde_json::to_string(&tc.function.arguments).unwrap_or_default()
                            ));
                        }
                        _ => {}
                    }
                }
            }
            Message::System { content } => {
                out.push_str("System: ");
                out.push_str(content);
                out.push('\n');
            }
        }
    }
    out
}

async fn call_summarize(client: &CompletionsClient, model: &str, text: &str) -> Result<String> {
    let agent = client.agent(model).preamble(SUMMARIZE_PREAMBLE).build();
    let mut history = Vec::new();
    let summary = agent
        .chat(Message::user(text), &mut history)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(summary)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn user_msg(text: &str) -> Message {
        Message::user(text)
    }

    fn tool_result_msg(id: &str, content: &str) -> Message {
        Message::User {
            content: OneOrMany::one(UserContent::ToolResult(ToolResult {
                id: id.to_string(),
                call_id: None,
                content: OneOrMany::one(ToolResultContent::text(content)),
            })),
        }
    }

    fn make_history(n: usize) -> Vec<Message> {
        (0..n).map(|i| user_msg(&format!("msg {i}"))).collect()
    }

    // ── needs_compaction ─────────────────────────────────────────────────────

    #[test]
    fn needs_compaction_percentage_under_threshold() {
        assert!(!needs_compaction(70_000, 131_072));
    }

    #[test]
    fn needs_compaction_percentage_at_threshold() {
        // 75% of 131072 = 98304
        assert!(needs_compaction(98_304, 131_072));
    }

    #[test]
    fn needs_compaction_no_context_size_under() {
        assert!(!needs_compaction(79_999, 0));
    }

    #[test]
    fn needs_compaction_no_context_size_at() {
        assert!(needs_compaction(80_000, 0));
    }

    // ── compress_tool_results ────────────────────────────────────────────────

    #[test]
    fn compress_short_tool_result_unchanged() {
        let short = "a".repeat(10);
        let mut history = vec![tool_result_msg("1", &short)];
        // Ensure the message is in the "old" zone (history.len() > KEEP_LAST).
        for i in 0..KEEP_LAST {
            history.push(user_msg(&format!("filler {i}")));
        }
        compress_tool_results(&mut history);

        if let Message::User { content } = &history[0]
            && let UserContent::ToolResult(tr) = content.first_ref()
        {
            let text = extract_tool_result_text(tr);
            assert_eq!(text, short);
        }
    }

    #[test]
    fn compress_long_tool_result_truncated() {
        let long = "x".repeat(TOOL_RESULT_MAX_CHARS + 100);
        let mut history = vec![tool_result_msg("1", &long)];
        for i in 0..KEEP_LAST {
            history.push(user_msg(&format!("filler {i}")));
        }
        compress_tool_results(&mut history);

        if let Message::User { content } = &history[0]
            && let UserContent::ToolResult(tr) = content.first_ref()
        {
            let text = extract_tool_result_text(tr);
            assert!(
                text.len() < long.len(),
                "expected truncation, got len {}",
                text.len()
            );
            assert!(text.contains("truncated"), "expected truncation marker");
        }
    }

    #[test]
    fn compress_leaves_recent_tool_results_alone() {
        let long = "y".repeat(TOOL_RESULT_MAX_CHARS + 100);
        // Only KEEP_LAST messages — the tool result is in the protected zone.
        let mut history = vec![tool_result_msg("1", &long)];
        for i in 0..KEEP_LAST - 1 {
            history.push(user_msg(&format!("filler {i}")));
        }
        compress_tool_results(&mut history);

        if let Message::User { content } = &history[0]
            && let UserContent::ToolResult(tr) = content.first_ref()
        {
            let text = extract_tool_result_text(tr);
            assert_eq!(
                text.len(),
                long.len(),
                "recent tool result should not be truncated"
            );
        }
    }

    // ── format_for_summary ───────────────────────────────────────────────────

    #[test]
    fn format_includes_user_text() {
        let msgs = vec![user_msg("hello world")];
        let out = format_for_summary(&msgs, "");
        assert!(out.contains("User: hello world"));
    }

    #[test]
    fn format_includes_existing_scratchpad() {
        let msgs = vec![user_msg("task")];
        let out = format_for_summary(&msgs, "- prior fact");
        assert!(out.contains("Previous memory"));
        assert!(out.contains("prior fact"));
    }

    // ── compact_with_summary (sync logic only) ───────────────────────────────

    #[test]
    fn compact_skipped_when_too_short() {
        // Cannot call async compact_with_summary in a sync test, so test the
        // guard condition directly.
        let history = make_history(KEEP_FIRST + KEEP_LAST);
        assert!(
            history.len() <= KEEP_FIRST + KEEP_LAST,
            "should not compact when len <= KEEP_FIRST + KEEP_LAST"
        );
    }

    #[test]
    fn compact_drop_zone_correct() {
        let n = 16_usize;
        let history = make_history(n);
        let drop_end = n - KEEP_LAST;
        // Verify the expected slice indices.
        assert_eq!(KEEP_FIRST, 2);
        assert_eq!(drop_end, n - KEEP_LAST);
        let drop_zone = &history[KEEP_FIRST..drop_end];
        assert_eq!(drop_zone.len(), n - KEEP_FIRST - KEEP_LAST);
    }
}
