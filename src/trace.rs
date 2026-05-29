use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{
    EnvFilter, Layer as _, layer::SubscriberExt as _, util::SubscriberInitExt as _,
};

/// Holds the non-blocking writer guards for all log sinks.
/// Must be kept alive for the entire process — drop it and buffered log lines
/// from either sink may be lost.
pub struct TraceGuard {
    _text: WorkerGuard,
    _json: WorkerGuard,
}

/// Initialise tracing to `~/.kapitoshka/`:
///   - `trace.log.<date>`  — human-readable text (daily rolling)
///   - `trace.json.<date>` — newline-delimited JSON for observability tools (daily rolling)
pub fn init() -> TraceGuard {
    let log_dir = std::env::var("HOME")
        .map(|h| format!("{h}/.kapitoshka"))
        .unwrap_or_else(|_| ".".to_string());

    let _ = std::fs::create_dir_all(&log_dir);

    let base_filter = || {
        EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| {
                EnvFilter::new(tracing::level_filters::LevelFilter::TRACE.to_string())
            })
            .add_directive("hyper=warn".parse().unwrap_or_default())
            .add_directive("hyper_util=warn".parse().unwrap_or_default())
            .add_directive("h2=warn".parse().unwrap_or_default())
            .add_directive("reqwest=warn".parse().unwrap_or_default())
            .add_directive("rustyline=warn".parse().unwrap_or_default())
            .add_directive("rustls=error".parse().unwrap_or_default())
    };

    // ── text layer ────────────────────────────────────────────────────────────
    let (text_writer, text_guard) =
        tracing_appender::non_blocking(tracing_appender::rolling::daily(&log_dir, "trace.log"));

    let text_layer = tracing_subscriber::fmt::layer()
        .with_writer(text_writer)
        .with_ansi(false)
        .with_file(false)
        .with_line_number(false)
        .with_thread_names(true)
        .with_target(true)
        .with_filter(base_filter());

    // ── JSON layer ────────────────────────────────────────────────────────────
    // Newline-delimited JSON; ingest directly into Loki, Datadog, or `jq`.
    let (json_writer, json_guard) =
        tracing_appender::non_blocking(tracing_appender::rolling::daily(&log_dir, "trace.json"));

    let json_layer = tracing_subscriber::fmt::layer()
        .json()
        .with_writer(json_writer)
        .with_current_span(true)
        .with_span_list(true)
        .with_filter(base_filter());

    tracing_subscriber::registry()
        .with(text_layer)
        .with(json_layer)
        .try_init()
        .unwrap_or(());

    TraceGuard {
        _text: text_guard,
        _json: json_guard,
    }
}
