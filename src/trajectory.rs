use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::fs::{File, OpenOptions};
use std::io::Write as IoWrite;
use std::path::PathBuf;
use std::time::Instant;

/// A single span within a turn — one atomic event in the trajectory.
#[derive(Debug, Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Span {
    Thinking {
        text: String,
    },
    ToolCall {
        id: String,
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        input: Option<serde_json::Value>,
    },
    ToolResult {
        /// Matches the id from the corresponding ToolCall span.
        call_id: String,
        output: String,
        duration_ms: u64,
    },
    Response {
        text: String,
        input_tokens: u64,
        output_tokens: u64,
        cached_tokens: u64,
        reasoning_tokens: u64,
    },
}

/// One full turn: user message + all spans produced by the agent.
#[derive(Debug, Serialize, serde::Deserialize)]
pub struct TurnRecord {
    pub turn: usize,
    pub timestamp: DateTime<Utc>,
    pub user: String,
    pub spans: Vec<Span>,
    pub total_duration_ms: u64,
}

/// Collects spans during a single agent turn and flushes a JSON record on
/// completion. Records are appended as newline-delimited JSON (`.jsonl`).
pub struct TrajectoryRecorder {
    file: File,
    turn: usize,
    user: String,
    spans: Vec<Span>,
    turn_start: Instant,
    /// Tracks the wall-clock start of the most recent tool call so we can
    /// compute its duration when the result arrives.
    pending_call: Option<(String, Instant)>,
    thinking_buf: String,
}

impl TrajectoryRecorder {
    pub fn new(session_path: &std::path::Path) -> Result<Self> {
        let path: PathBuf = session_path.with_extension("jsonl");
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self {
            file,
            turn: 0,
            user: String::new(),
            spans: Vec::new(),
            turn_start: Instant::now(),
            pending_call: None,
            thinking_buf: String::new(),
        })
    }

    /// Start a new turn with the given user message.
    pub fn start_turn(&mut self, user: &str) {
        self.turn += 1;
        self.user = user.to_owned();
        self.spans.clear();
        self.turn_start = Instant::now();
        self.pending_call = None;
        self.thinking_buf.clear();
    }

    /// Accumulate a thinking chunk; flushed as a single span on `finish_thinking`.
    pub fn feed_thinking(&mut self, chunk: &str) {
        self.thinking_buf.push_str(chunk);
    }

    /// Emit the buffered thinking content as a span (if non-empty).
    pub fn finish_thinking(&mut self) {
        let text = std::mem::take(&mut self.thinking_buf);
        if !text.is_empty() {
            self.spans.push(Span::Thinking { text });
        }
    }

    /// Record the start of a tool call. The duration clock starts here.
    pub fn tool_call(&mut self, id: &str, name: &str, input: Option<serde_json::Value>) {
        self.pending_call = Some((id.to_owned(), Instant::now()));
        self.spans.push(Span::ToolCall {
            id: id.to_owned(),
            name: name.to_owned(),
            input,
        });
    }

    /// Record the tool result. Matches the pending call by id and records elapsed time.
    pub fn tool_result(&mut self, call_id: &str, output: &str) {
        let duration_ms = self
            .pending_call
            .take()
            .map(|(_, t)| t.elapsed().as_millis() as u64)
            .unwrap_or(0);
        self.spans.push(Span::ToolResult {
            call_id: call_id.to_owned(),
            output: output.to_owned(),
            duration_ms,
        });
    }

    /// Record the final response and flush the complete turn record to disk.
    pub fn finish_turn(
        &mut self,
        text: &str,
        input_tokens: u64,
        output_tokens: u64,
        cached_tokens: u64,
        reasoning_tokens: u64,
    ) -> Result<()> {
        self.spans.push(Span::Response {
            text: text.to_owned(),
            input_tokens,
            output_tokens,
            cached_tokens,
            reasoning_tokens,
        });

        let record = TurnRecord {
            turn: self.turn,
            timestamp: Utc::now(),
            user: self.user.clone(),
            spans: std::mem::take(&mut self.spans),
            total_duration_ms: self.turn_start.elapsed().as_millis() as u64,
        };

        let mut line = serde_json::to_string(&record)?;
        line.push('\n');
        self.file.write_all(line.as_bytes())?;
        self.file.flush()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn recorder_in(dir: &TempDir) -> TrajectoryRecorder {
        let path = dir.path().join("session.md");
        TrajectoryRecorder::new(&path).expect("recorder creation failed")
    }

    fn read_records(dir: &TempDir) -> Vec<serde_json::Value> {
        let jsonl = dir.path().join("session.jsonl");
        let content = std::fs::read_to_string(jsonl).unwrap_or_default();
        content
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| serde_json::from_str(l).expect("invalid JSON line"))
            .collect()
    }

    // ── basic turn flush ─────────────────────────────────────────────────────

    #[test]
    fn empty_turn_writes_one_record() {
        let dir = TempDir::new().unwrap();
        let mut r = recorder_in(&dir);
        r.start_turn("hello");
        r.finish_turn("world", 10, 5, 2, 0).unwrap();

        let records = read_records(&dir);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["turn"], 1);
        assert_eq!(records[0]["user"], "hello");
    }

    #[test]
    fn multiple_turns_append_multiple_records() {
        let dir = TempDir::new().unwrap();
        let mut r = recorder_in(&dir);

        for i in 1..=3 {
            r.start_turn(&format!("q{i}"));
            r.finish_turn(&format!("a{i}"), 0, 0, 0, 0).unwrap();
        }

        let records = read_records(&dir);
        assert_eq!(records.len(), 3);
        assert_eq!(records[0]["turn"], 1);
        assert_eq!(records[1]["turn"], 2);
        assert_eq!(records[2]["turn"], 3);
    }

    // ── response span ────────────────────────────────────────────────────────

    #[test]
    fn response_span_carries_token_counts() {
        let dir = TempDir::new().unwrap();
        let mut r = recorder_in(&dir);
        r.start_turn("q");
        r.finish_turn("ans", 100, 50, 30, 10).unwrap();

        let records = read_records(&dir);
        let spans = records[0]["spans"].as_array().unwrap();
        let resp = spans.iter().find(|s| s["type"] == "response").unwrap();
        assert_eq!(resp["input_tokens"], 100);
        assert_eq!(resp["output_tokens"], 50);
        assert_eq!(resp["cached_tokens"], 30);
        assert_eq!(resp["reasoning_tokens"], 10);
        assert_eq!(resp["text"], "ans");
    }

    // ── thinking spans ───────────────────────────────────────────────────────

    #[test]
    fn thinking_buffered_into_single_span() {
        let dir = TempDir::new().unwrap();
        let mut r = recorder_in(&dir);
        r.start_turn("q");
        r.feed_thinking("chunk1 ");
        r.feed_thinking("chunk2");
        r.finish_thinking();
        r.finish_turn("ans", 0, 0, 0, 0).unwrap();

        let records = read_records(&dir);
        let spans = records[0]["spans"].as_array().unwrap();
        let thinking: Vec<_> = spans.iter().filter(|s| s["type"] == "thinking").collect();
        assert_eq!(thinking.len(), 1);
        assert_eq!(thinking[0]["text"], "chunk1 chunk2");
    }

    #[test]
    fn empty_thinking_buffer_produces_no_span() {
        let dir = TempDir::new().unwrap();
        let mut r = recorder_in(&dir);
        r.start_turn("q");
        r.finish_thinking(); // nothing fed
        r.finish_turn("ans", 0, 0, 0, 0).unwrap();

        let records = read_records(&dir);
        let spans = records[0]["spans"].as_array().unwrap();
        assert!(!spans.iter().any(|s| s["type"] == "thinking"));
    }

    #[test]
    fn thinking_buffer_cleared_between_turns() {
        let dir = TempDir::new().unwrap();
        let mut r = recorder_in(&dir);

        r.start_turn("q1");
        r.feed_thinking("turn1 thought");
        r.finish_thinking();
        r.finish_turn("a1", 0, 0, 0, 0).unwrap();

        r.start_turn("q2");
        // no thinking fed this turn
        r.finish_turn("a2", 0, 0, 0, 0).unwrap();

        let records = read_records(&dir);
        let spans2 = records[1]["spans"].as_array().unwrap();
        assert!(!spans2.iter().any(|s| s["type"] == "thinking"));
    }

    // ── tool call / result spans ─────────────────────────────────────────────

    #[test]
    fn tool_call_span_recorded() {
        let dir = TempDir::new().unwrap();
        let mut r = recorder_in(&dir);
        r.start_turn("q");
        r.tool_call(
            "id1",
            "read_file",
            Some(serde_json::json!({"path": "a.txt"})),
        );
        r.tool_result("id1", "file contents");
        r.finish_turn("ans", 0, 0, 0, 0).unwrap();

        let records = read_records(&dir);
        let spans = records[0]["spans"].as_array().unwrap();

        let call = spans.iter().find(|s| s["type"] == "tool_call").unwrap();
        assert_eq!(call["id"], "id1");
        assert_eq!(call["name"], "read_file");
        assert_eq!(call["input"]["path"], "a.txt");

        let result = spans.iter().find(|s| s["type"] == "tool_result").unwrap();
        assert_eq!(result["call_id"], "id1");
        assert_eq!(result["output"], "file contents");
        assert!(result["duration_ms"].as_u64().is_some());
    }

    #[test]
    fn tool_call_without_input_omits_field() {
        let dir = TempDir::new().unwrap();
        let mut r = recorder_in(&dir);
        r.start_turn("q");
        r.tool_call("id2", "list_dir", None);
        r.tool_result("id2", "[]");
        r.finish_turn("ans", 0, 0, 0, 0).unwrap();

        let records = read_records(&dir);
        let spans = records[0]["spans"].as_array().unwrap();
        let call = spans.iter().find(|s| s["type"] == "tool_call").unwrap();
        assert!(call.get("input").is_none() || call["input"].is_null());
    }

    #[test]
    fn multiple_tool_calls_in_one_turn() {
        let dir = TempDir::new().unwrap();
        let mut r = recorder_in(&dir);
        r.start_turn("q");
        r.tool_call("c1", "read_file", None);
        r.tool_result("c1", "content1");
        r.tool_call("c2", "run_shell", Some(serde_json::json!({"cmd": "ls"})));
        r.tool_result("c2", "a.txt");
        r.finish_turn("done", 0, 0, 0, 0).unwrap();

        let records = read_records(&dir);
        let spans = records[0]["spans"].as_array().unwrap();
        let calls: Vec<_> = spans.iter().filter(|s| s["type"] == "tool_call").collect();
        let results: Vec<_> = spans
            .iter()
            .filter(|s| s["type"] == "tool_result")
            .collect();
        assert_eq!(calls.len(), 2);
        assert_eq!(results.len(), 2);
        assert_eq!(calls[0]["id"], "c1");
        assert_eq!(calls[1]["id"], "c2");
    }

    // ── span ordering ────────────────────────────────────────────────────────

    #[test]
    fn spans_appear_in_emission_order() {
        let dir = TempDir::new().unwrap();
        let mut r = recorder_in(&dir);
        r.start_turn("q");
        r.feed_thinking("thought");
        r.finish_thinking();
        r.tool_call("t1", "read_file", None);
        r.tool_result("t1", "data");
        r.finish_turn("reply", 0, 0, 0, 0).unwrap();

        let records = read_records(&dir);
        let types: Vec<&str> = records[0]["spans"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["type"].as_str().unwrap())
            .collect();
        assert_eq!(types, ["thinking", "tool_call", "tool_result", "response"]);
    }

    // ── metadata ─────────────────────────────────────────────────────────────

    #[test]
    fn total_duration_ms_is_non_negative() {
        let dir = TempDir::new().unwrap();
        let mut r = recorder_in(&dir);
        r.start_turn("q");
        r.finish_turn("a", 0, 0, 0, 0).unwrap();

        let records = read_records(&dir);
        assert!(records[0]["total_duration_ms"].as_u64().is_some());
    }

    #[test]
    fn timestamp_is_valid_rfc3339() {
        let dir = TempDir::new().unwrap();
        let mut r = recorder_in(&dir);
        r.start_turn("q");
        r.finish_turn("a", 0, 0, 0, 0).unwrap();

        let records = read_records(&dir);
        let ts = records[0]["timestamp"].as_str().unwrap();
        assert!(
            ts.parse::<DateTime<Utc>>().is_ok(),
            "not valid RFC-3339: {ts}"
        );
    }

    // ── persistence ──────────────────────────────────────────────────────────

    #[test]
    fn appends_to_existing_file() {
        let dir = TempDir::new().unwrap();

        {
            let mut r = recorder_in(&dir);
            r.start_turn("first");
            r.finish_turn("a", 0, 0, 0, 0).unwrap();
        }
        // New recorder instance, same path — simulates resume.
        {
            let mut r = recorder_in(&dir);
            r.start_turn("second");
            r.finish_turn("b", 0, 0, 0, 0).unwrap();
        }

        let records = read_records(&dir);
        assert_eq!(records.len(), 2);
    }
}
