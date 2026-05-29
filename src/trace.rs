use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::WithExportConfig as _;
use opentelemetry_sdk::trace::SdkTracerProvider;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{
    EnvFilter, Layer as _, layer::SubscriberExt as _, util::SubscriberInitExt as _,
};

/// Holds the non-blocking writer guards for all log sinks and the OTel provider.
/// Must be kept alive for the entire process — drop it and buffered log lines
/// or in-flight spans may be lost.
pub struct TraceGuard {
    _text: WorkerGuard,
    _json: WorkerGuard,
    otel: Option<SdkTracerProvider>,
}

impl Drop for TraceGuard {
    fn drop(&mut self) {
        if let Some(provider) = self.otel.take() {
            // Flush all in-flight spans before the process exits.
            if let Err(e) = provider.shutdown() {
                eprintln!("otel shutdown error: {e}");
            }
        }
    }
}

/// Initialise tracing to `~/.kapitoshka/`:
///   - `trace.log.<date>`  — human-readable text (daily rolling)
///   - `trace.json.<date>` — newline-delimited JSON for observability tools (daily rolling)
///
/// When `OTEL_EXPORTER_OTLP_ENDPOINT` is set, spans are also exported via OTLP/gRPC.
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
    let (json_writer, json_guard) =
        tracing_appender::non_blocking(tracing_appender::rolling::daily(&log_dir, "trace.json"));

    let json_layer = tracing_subscriber::fmt::layer()
        .json()
        .with_writer(json_writer)
        .with_current_span(true)
        .with_span_list(true)
        .with_filter(base_filter());

    // ── OTLP layer (optional) ─────────────────────────────────────────────────
    // Activated when OTEL_EXPORTER_OTLP_ENDPOINT is set, e.g.:
    //   OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317 kapitoshka …
    let (otel_layer, otel_provider) = build_otel_layer();
    // Box the layer so `Option<Box<dyn Layer<_>>>` unifies regardless of how
    // deep the `Layered<…>` subscriber type has grown by this point.
    let otel_layer = otel_layer.map(tracing_subscriber::Layer::boxed);

    // OTel layer must be added first so Option<Box<dyn Layer<Registry>>>
    // resolves against the bare Registry before the Layered<…> type grows.
    tracing_subscriber::registry()
        .with(otel_layer)
        .with(text_layer)
        .with(json_layer)
        .try_init()
        .unwrap_or(());

    TraceGuard {
        _text: text_guard,
        _json: json_guard,
        otel: otel_provider,
    }
}

type OtelLayer = Option<
    tracing_opentelemetry::OpenTelemetryLayer<
        tracing_subscriber::Registry,
        opentelemetry_sdk::trace::Tracer,
    >,
>;

fn build_otel_layer() -> (OtelLayer, Option<SdkTracerProvider>) {
    let endpoint = match std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT") {
        Ok(e) if !e.is_empty() => e,
        _ => return (None, None),
    };

    let service_name =
        std::env::var("OTEL_SERVICE_NAME").unwrap_or_else(|_| "kapitoshka".to_string());

    let exporter = match opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(&endpoint)
        .build()
    {
        Ok(e) => e,
        Err(err) => {
            eprintln!("otel exporter init failed ({endpoint}): {err}");
            return (None, None);
        }
    };

    let provider = SdkTracerProvider::builder()
        .with_resource(
            opentelemetry_sdk::Resource::builder_empty()
                .with_attribute(opentelemetry::KeyValue::new("service.name", service_name))
                .build(),
        )
        .with_batch_exporter(exporter)
        .build();

    let tracer = provider.tracer("kapitoshka");
    let layer = tracing_opentelemetry::layer().with_tracer(tracer);

    (Some(layer), Some(provider))
}
