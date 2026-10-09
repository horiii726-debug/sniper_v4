//! Structured logging with mandatory secret redaction (V4 §30). Every log line passes through
//! `Redactor` before it reaches stderr or the log file. Warnings/errors emitted by the LaserStream
//! SDK (which reconnects silently) are also forwarded into the feed as `grpc_stream_error` markers.

use config::Redactor;
use events::RawMsg;
use std::fs::File;
use std::io::Write;
use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::mpsc;
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

pub static SDK_EVENTS: OnceLock<mpsc::Sender<RawMsg>> = OnceLock::new();

#[derive(Clone)]
struct RedactingMakeWriter {
    file: Option<Arc<Mutex<File>>>,
    red: Arc<Redactor>,
}

struct LineBuf {
    buf: Vec<u8>,
    file: Option<Arc<Mutex<File>>>,
    red: Arc<Redactor>,
}

impl Write for LineBuf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.buf.extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Drop for LineBuf {
    fn drop(&mut self) {
        let clean = self.red.redact(&String::from_utf8_lossy(&self.buf));
        let _ = std::io::stderr().write_all(clean.as_bytes());
        if let Some(f) = &self.file {
            if let Ok(mut f) = f.lock() {
                let _ = f.write_all(clean.as_bytes());
            }
        }
    }
}

impl<'a> MakeWriter<'a> for RedactingMakeWriter {
    type Writer = LineBuf;
    fn make_writer(&'a self) -> LineBuf {
        LineBuf { buf: Vec::with_capacity(256), file: self.file.clone(), red: self.red.clone() }
    }
}

struct SdkCapture;

impl<S: Subscriber> Layer<S> for SdkCapture {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let md = event.metadata();
        if md.target().starts_with("helius_laserstream") && *md.level() <= Level::WARN {
            if let Some(tx) = SDK_EVENTS.get() {
                let _ = tx.try_send(feed::control("grpc_stream_error", serde_json::json!({ "level": md.level().as_str() })));
            }
        }
    }
}

pub fn init(red: Redactor, log_file: Option<&std::path::Path>) -> anyhow::Result<()> {
    let file = match log_file {
        Some(p) => {
            if let Some(d) = p.parent() {
                std::fs::create_dir_all(d)?;
            }
            Some(Arc::new(Mutex::new(std::fs::OpenOptions::new().create(true).append(true).open(p)?)))
        }
        None => None,
    };
    let red = Arc::new(red);
    let writer = RedactingMakeWriter { file, red: red.clone() };
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(tracing_subscriber::fmt::layer().with_ansi(false).with_writer(writer))
        .with(SdkCapture)
        .try_init()?;
    std::panic::set_hook(Box::new(move |info| {
        let msg = red.redact(&info.to_string());
        tracing::error!(panic = %msg, "PANIC");
    }));
    Ok(())
}
