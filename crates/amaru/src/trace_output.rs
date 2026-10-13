// Copyright 2026 PRAGMA
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Extra trace files requested on the command line.
//!
//! Each spec is `PATH:FILTER`, split at the first colon. A path ending in `.ndjson` uses the
//! same JSON event format as `--with-json-traces`, one object per line; any other path is the
//! console text format. Span close lines include busy and idle time. These layers are added
//! beside the terminal, TUI, and OpenTelemetry outputs, each with its own filter.

use std::{
    fs::File,
    io::{self, Write},
    path::{Path, PathBuf},
    str::FromStr,
    sync::{Arc, Mutex},
};

use amaru_observability::{
    CborConsoleEventFormat, CborJsonEventFormat, CborJsonFields, console_field_formatter,
    tracing::Subscriber,
    tracing_subscriber::{
        EnvFilter, Layer,
        fmt::{self, MakeWriter, format::FmtSpan},
        layer::{Layered, SubscriberExt},
        registry::LookupSpan,
    },
};
use anyhow::Context;

/// One `--log-output` value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceOutputSpec {
    path: PathBuf,
    filter: String,
    json: bool,
}

impl TraceOutputSpec {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn filter(&self) -> &str {
        &self.filter
    }

    /// `true` when the file name ends in `.ndjson`.
    pub fn is_json(&self) -> bool {
        self.json
    }
}

impl FromStr for TraceOutputSpec {
    type Err = String;

    fn from_str(spec: &str) -> Result<Self, Self::Err> {
        #[cfg(windows)]
        fn split(s: &str) -> Option<(&str, &str)> {
            let mut chars = s.chars();
            if chars.next().is_some_and(|c| c.is_ascii_alphabetic()) && chars.next().is_some_and(|c| c == ':') {
                let pos = s[2..].find(':')? + 2;
                Some((&s[..pos], &s[pos + 1..]))
            } else {
                s.split_once(':')
            }
        }
        #[cfg(not(windows))]
        fn split(s: &str) -> Option<(&str, &str)> {
            s.split_once(':')
        }

        let Some((path, filter)) = split(spec) else {
            return Err(format!(
                "log output `{spec}` must be PATH:FILTER, split at the first colon (for example effects.ndjson:amaru_pure_stage::effect=debug)"
            ));
        };
        if path.is_empty() {
            return Err(format!("log output `{spec}` is missing a path before the first colon"));
        }
        if filter.is_empty() {
            return Err(format!("log output `{spec}` is missing a filter after the first colon"));
        }
        EnvFilter::try_new(filter).map_err(|err| format!("log output `{spec}` has an invalid filter: {err}"))?;
        Ok(Self { path: PathBuf::from(path), filter: filter.to_string(), json: path.ends_with(".ndjson") })
    }
}

/// Stack one fmt layer per spec onto `subscriber`. An empty list leaves `subscriber` unchanged.
pub(crate) fn attach<S>(
    subscriber: S,
    outputs: &[TraceOutputSpec],
) -> anyhow::Result<Layered<Option<Vec<Box<dyn Layer<S> + Send + Sync + 'static>>>, S>>
where
    S: Subscriber + for<'a> LookupSpan<'a> + 'static,
{
    if outputs.is_empty() {
        return Ok(subscriber.with(None));
    }
    let mut layers = Vec::with_capacity(outputs.len());
    for output in outputs {
        layers.push(output.layer()?);
    }
    Ok(subscriber.with(Some(layers)))
}

impl TraceOutputSpec {
    fn layer<S>(&self) -> anyhow::Result<Box<dyn Layer<S> + Send + Sync + 'static>>
    where
        S: Subscriber + for<'a> LookupSpan<'a> + 'static,
    {
        let file =
            File::create(&self.path).with_context(|| format!("failed to open log output {}", self.path.display()))?;
        let writer = TraceFileWriter { file: Arc::new(Mutex::new(file)) };
        let filter = EnvFilter::try_new(&self.filter)
            .with_context(|| format!("invalid filter for log output {}", self.path.display()))?;
        // CLOSE emits one event per span with time.busy and time.idle.
        if self.json {
            Ok(fmt::layer()
                .with_writer(writer)
                .with_span_events(FmtSpan::CLOSE)
                .event_format(CborJsonEventFormat::new())
                .fmt_fields(CborJsonFields::new())
                .with_filter(filter)
                .boxed())
        } else {
            Ok(fmt::layer()
                .with_writer(writer)
                .with_ansi(false)
                .fmt_fields(console_field_formatter())
                .with_span_events(FmtSpan::CLOSE)
                .event_format(CborConsoleEventFormat::new().with_ansi(false))
                .with_filter(filter)
                .boxed())
        }
    }
}

struct TraceFileWriter<W> {
    file: Arc<Mutex<W>>,
}

impl<W> Clone for TraceFileWriter<W> {
    fn clone(&self) -> Self {
        Self { file: Arc::clone(&self.file) }
    }
}

impl<W: Write> Write for TraceFileWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut file = self.file.lock().map_err(|err| io::Error::other(err.to_string()))?;
        let written = file.write(buf)?;
        // `write` may accept only a prefix. A newline past that prefix is not in the file yet.
        if buf[..written].contains(&b'\n') {
            file.flush()?;
        }
        Ok(written)
    }

    fn write_all(&mut self, mut buf: &[u8]) -> io::Result<()> {
        // The formatter writes one event with `write_all`. Hold the mutex across every short
        // write so another thread cannot insert bytes into the middle of the record.
        let mut file = self.file.lock().map_err(|err| io::Error::other(err.to_string()))?;
        let flush_after = buf.contains(&b'\n');
        while !buf.is_empty() {
            match file.write(buf) {
                Ok(0) => {
                    return Err(io::Error::new(io::ErrorKind::WriteZero, "failed to write the whole trace record"));
                }
                Ok(written) => buf = &buf[written..],
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                Err(err) => return Err(err),
            }
        }
        if flush_after {
            file.flush()?;
        }
        Ok(())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.lock().map_err(|err| io::Error::other(err.to_string()))?.flush()
    }
}

impl<'a, W> MakeWriter<'a> for TraceFileWriter<W>
where
    W: Write + Send + 'static,
{
    type Writer = TraceFileWriter<W>;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use amaru_observability::tracing_subscriber;
    use amaru_pure_stage::EFFECT_SPAN_TARGET;
    use serde_json::Value;
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn splits_at_the_first_colon_and_keeps_module_paths() {
        let spec = "effects.ndjson:amaru_pure_stage::effect=debug,amaru_pure_stage::tokio=trace"
            .parse::<TraceOutputSpec>()
            .expect("spec");
        assert_eq!(spec.path(), Path::new("effects.ndjson"));
        assert_eq!(spec.filter(), "amaru_pure_stage::effect=debug,amaru_pure_stage::tokio=trace");
        assert!(spec.is_json());
    }

    #[test]
    fn text_file_is_not_json() {
        let spec = "effects.log:amaru_pure_stage::effect=debug".parse::<TraceOutputSpec>().expect("spec");
        assert_eq!(spec.path(), Path::new("effects.log"));
        assert!(!spec.is_json());
    }

    #[test]
    fn rejects_a_spec_without_a_colon() {
        let err = "effects.ndjson".parse::<TraceOutputSpec>().expect_err("missing colon");
        assert!(err.contains("PATH:FILTER"), "{err}");
        assert!(err.contains("effects.ndjson:amaru_pure_stage::effect=debug"), "{err}");
    }

    #[test]
    fn rejects_an_empty_path_or_filter() {
        let missing_path = ":amaru_pure_stage::effect=debug".parse::<TraceOutputSpec>().expect_err("path");
        assert!(missing_path.contains("missing a path"), "{missing_path}");
        let missing_filter = "effects.ndjson:".parse::<TraceOutputSpec>().expect_err("filter");
        assert!(missing_filter.contains("missing a filter"), "{missing_filter}");
    }

    #[test]
    fn rejects_an_invalid_filter() {
        let err = "effects.ndjson:???".parse::<TraceOutputSpec>().expect_err("filter");
        assert!(err.contains("invalid filter"), "{err}");
    }

    #[test]
    fn ndjson_file_records_an_effect_span_close() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("effects.ndjson");
        let spec =
            format!("{}:amaru_pure_stage::effect=debug", path.display()).parse::<TraceOutputSpec>().expect("spec");
        assert!(spec.is_json());

        let subscriber = attach(tracing_subscriber::registry(), &[spec]).expect("layer");
        amaru_observability::tracing::subscriber::with_default(subscriber, || {
            let span = amaru_observability::tracing::debug_span!(
                target: EFFECT_SPAN_TARGET,
                "effect",
                type_name = "demo::MeasuredEffect",
                stage = "worker-1",
            );
            let _entered = span.enter();
            std::thread::sleep(Duration::from_millis(1));
        });

        let text = std::fs::read_to_string(path).expect("read");
        let lines: Vec<_> = text.lines().filter(|line| !line.is_empty()).collect();
        assert_eq!(lines.len(), 1, "one close object, got {text}");
        let value: Value = serde_json::from_str(lines[0]).expect("json");
        let fields = value.get("fields").expect("fields");
        assert_eq!(fields.get("message").and_then(Value::as_str), Some("close"));
        assert!(fields.get("time.busy").is_some(), "{value}");
        assert!(fields.get("time.idle").is_some(), "{value}");
        // Same envelope as `--with-json-traces`: span fields are inlined, and `span` is only name and target.
        assert_eq!(fields.get("type_name").and_then(Value::as_str), Some("demo::MeasuredEffect"));
        assert_eq!(fields.get("stage").and_then(Value::as_str), Some("worker-1"));
        assert_eq!(value["span"]["name"], "effect");
        assert_eq!(value["span"]["target"], EFFECT_SPAN_TARGET);
        assert!(value["span"].get("type_name").is_none(), "{value}");
        assert_eq!(value["parents"], serde_json::json!([]));
        assert!(value.get("spans").is_none(), "{value}");
    }

    #[test]
    fn flush_follows_only_the_bytes_actually_written() {
        #[derive(Default)]
        struct LimitedWriter {
            buf: Vec<u8>,
            limit: usize,
            flushes: u32,
        }

        impl Write for LimitedWriter {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                let n = buf.len().min(self.limit);
                self.buf.extend_from_slice(&buf[..n]);
                Ok(n)
            }

            fn flush(&mut self) -> io::Result<()> {
                self.flushes += 1;
                Ok(())
            }
        }

        let mut writer =
            TraceFileWriter { file: Arc::new(Mutex::new(LimitedWriter { limit: 4, ..LimitedWriter::default() })) };
        // The newline sits past the four bytes this writer accepts.
        let n = writer.write(b"abcd\n").expect("write");
        assert_eq!(n, 4);
        {
            let guard = writer.file.lock().expect("lock");
            assert_eq!(guard.flushes, 0, "a newline that was not written must not flush");
            assert_eq!(guard.buf, b"abcd");
        }

        writer.file.lock().expect("lock").limit = 2;
        let n = writer.write(b"\n!").expect("write");
        assert_eq!(n, 2);
        let guard = writer.file.lock().expect("lock");
        assert_eq!(guard.flushes, 1, "a newline inside the accepted prefix must flush");
        assert_eq!(guard.buf, b"abcd\n!");
    }

    #[test]
    fn write_all_keeps_a_short_write_inside_one_record() {
        #[derive(Default)]
        struct LimitedWriter {
            buf: Vec<u8>,
            limit: usize,
            flushes: u32,
        }

        impl Write for LimitedWriter {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                let n = buf.len().min(self.limit);
                self.buf.extend_from_slice(&buf[..n]);
                Ok(n)
            }

            fn flush(&mut self) -> io::Result<()> {
                self.flushes += 1;
                Ok(())
            }
        }

        let mut writer =
            TraceFileWriter { file: Arc::new(Mutex::new(LimitedWriter { limit: 3, ..LimitedWriter::default() })) };
        // Two chunks each contain a newline. One `write_all` is still one record.
        writer.write_all(b"ab\ncd\n").expect("write_all");
        {
            let guard = writer.file.lock().expect("lock");
            assert_eq!(guard.buf, b"ab\ncd\n");
            assert_eq!(guard.flushes, 1, "the whole record is flushed once");
        }

        let mut plain =
            TraceFileWriter { file: Arc::new(Mutex::new(LimitedWriter { limit: 3, ..LimitedWriter::default() })) };
        plain.write_all(b"abcdef").expect("write_all");
        let guard = plain.file.lock().expect("lock");
        assert_eq!(guard.buf, b"abcdef");
        assert_eq!(guard.flushes, 0, "a buffer with no newline is not flushed");
    }

    #[test]
    fn text_file_records_busy_time() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("effects.log");
        let spec =
            format!("{}:amaru_pure_stage::effect=debug", path.display()).parse::<TraceOutputSpec>().expect("spec");
        assert!(!spec.is_json());

        let subscriber = attach(tracing_subscriber::registry(), &[spec]).expect("layer");
        amaru_observability::tracing::subscriber::with_default(subscriber, || {
            let span = amaru_observability::tracing::debug_span!(
                target: EFFECT_SPAN_TARGET,
                "effect",
                type_name = "demo::MeasuredEffect",
            );
            drop(span.enter());
        });

        let text = std::fs::read_to_string(path).expect("read");
        assert!(text.contains("close"), "{text}");
        assert!(text.contains("time.busy"), "{text}");
        assert!(text.contains("demo::MeasuredEffect"), "{text}");
    }
}
