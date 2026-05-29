use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{
    EnvFilter, Layer as _, layer::SubscriberExt as _, util::SubscriberInitExt as _,
};

/// Initialise tracing to `~/.kapitoshka/trace.log` so the terminal UI is not
/// polluted. The returned `WorkerGuard` must be kept alive for the entire
/// process — drop it and the background writer may lose buffered messages.
pub fn init() -> WorkerGuard {
    let log_dir = std::env::var("HOME")
        .map(|h| format!("{h}/.kapitoshka"))
        .unwrap_or_else(|_| ".".to_string());

    let _ = std::fs::create_dir_all(&log_dir);

    let file_appender = tracing_appender::rolling::never(&log_dir, "trace.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(tracing::level_filters::LevelFilter::TRACE.to_string()))
        .add_directive("hyper_util=warn".parse().unwrap_or_default())
        .add_directive("rustyline=warn".parse().unwrap_or_default())
        .add_directive("rustls=error".parse().unwrap_or_default());

    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(non_blocking)
        .with_ansi(false)
        .with_file(false)
        .with_line_number(false)
        .with_thread_names(true)
        .with_target(true)
        .with_filter(env_filter);

    tracing_subscriber::registry()
        .with(file_layer)
        .try_init()
        .unwrap_or(());

    guard
}
