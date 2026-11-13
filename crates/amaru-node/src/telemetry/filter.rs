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

//! Environment filters and grouped OTLP export failure reporting.

use std::{
    env::VarError,
    error::Error,
    sync::{OnceLock, mpsc},
    time::{Duration, Instant},
};

use amaru_observability::{info, warn};
use tracing::{Metadata, Subscriber, level_filters::LevelFilter, span, subscriber::Interest};
use tracing_subscriber::{
    EnvFilter,
    layer::{Context, Filter},
};

use super::OtelSignal;

/// A startup notice that must run after the subscriber has been installed.
pub type StartupNotice = Box<dyn FnOnce()>;

const OTEL_EXPORT_TRANSITION_DELAY: Duration = Duration::from_secs(5);
static OTEL_EXPORT_REPORTER: OnceLock<OtelExportReporter> = OnceLock::new();

/// Wraps an [`EnvFilter`] and replaces repeated OpenTelemetry SDK errors with
/// one actionable Amaru warning.
pub(crate) struct OtelErrorFilter {
    inner: EnvFilter,
    reporter: Option<OtelExportReporter>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OtelExportTransition {
    Failed(OtelSignal),
    Recovered(OtelSignal),
}

impl OtelErrorFilter {
    fn new(inner: EnvFilter, reporter: Option<OtelExportReporter>) -> Self {
        Self { inner, reporter }
    }

    pub(crate) fn wrap(inner: EnvFilter, report_export_errors: bool) -> Self {
        let reporter =
            report_export_errors.then(|| OTEL_EXPORT_REPORTER.get_or_init(OtelExportReporter::start).clone());
        Self::new(inner, reporter)
    }

    fn is_otel_export_error(meta: &Metadata<'_>) -> bool {
        meta.level() == &amaru_observability::tracing::Level::ERROR
            && meta.target().starts_with("opentelemetry")
            && (meta.name().ends_with("ExportError") || meta.name().ends_with("ExportFailed"))
    }

    fn is_otel_export_success(meta: &Metadata<'_>) -> bool {
        meta.level() == &amaru_observability::tracing::Level::DEBUG
            && meta.target().starts_with("opentelemetry")
            && meta.name().ends_with("ExportSucceeded")
    }

    fn otel_signal(meta: &Metadata<'_>) -> OtelSignal {
        if meta.name().contains("Log") {
            OtelSignal::Logs
        } else if meta.name().contains("Span") || meta.name().contains("Trace") {
            OtelSignal::Traces
        } else {
            OtelSignal::Metrics
        }
    }

    fn report_otel_export_failure(&self, meta: &Metadata<'_>) {
        if let Some(reporter) = &self.reporter {
            reporter.report(OtelExportTransition::Failed(Self::otel_signal(meta)));
        }
    }

    fn report_otel_export_recovery(&self, meta: &Metadata<'_>) {
        if let Some(reporter) = &self.reporter {
            reporter.report(OtelExportTransition::Recovered(Self::otel_signal(meta)));
        }
    }
}

#[derive(Debug, Clone)]
struct OtelExportReporter {
    /// The reporter thread owns all mutable transition state; filters only send observations.
    sender: mpsc::Sender<OtelExportTransition>,
}

impl OtelExportReporter {
    fn start() -> Self {
        let (sender, receiver) = mpsc::channel();
        let _reporter_thread = std::thread::Builder::new()
            .name("amaru-otel-reporter".into())
            .spawn(move || run_otel_export_reporter(receiver));
        Self { sender }
    }

    fn report(&self, transition: OtelExportTransition) {
        let _ = self.sender.send(transition);
    }
}

fn run_otel_export_reporter(receiver: mpsc::Receiver<OtelExportTransition>) {
    let mut current_failures = [false; 3];
    let mut reported_failures = [false; 3];
    let mut deadline: Option<Instant> = None;

    loop {
        let received = match deadline {
            Some(deadline) => receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())),
            None => match receiver.recv() {
                Ok(transition) => Ok(transition),
                Err(_) => break,
            },
        };

        match received {
            Ok(OtelExportTransition::Failed(signal)) => {
                current_failures[signal.index()] = true;
            }
            Ok(OtelExportTransition::Recovered(signal)) => {
                current_failures[signal.index()] = false;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }

        if current_failures == reported_failures {
            deadline = None;
            continue;
        }

        let now = Instant::now();
        let Some(report_at) = deadline else {
            deadline = Some(now + OTEL_EXPORT_TRANSITION_DELAY);
            continue;
        };
        if report_at > now {
            continue;
        }

        let (failures, recoveries) = otel_export_changes(&current_failures, &reported_failures);
        reported_failures = current_failures;
        deadline = None;

        if !failures.is_empty() {
            let unavailable_signals = otlp_signals(&failures);
            warn!(setup::observability::EXPORT_FAILED, unavailable_signals);
        }
        if !recoveries.is_empty() {
            let recovered_signals = otlp_signals(&recoveries);
            info!(setup::observability::EXPORT_RECOVERED, recovered_signals);
        }
    }
}

fn otel_export_changes(current: &[bool; 3], reported: &[bool; 3]) -> (Vec<OtelSignal>, Vec<OtelSignal>) {
    let has_new_failure =
        OtelSignal::ALL.into_iter().any(|signal| current[signal.index()] && !reported[signal.index()]);
    let failures = if has_new_failure {
        OtelSignal::ALL.into_iter().filter(|signal| current[signal.index()]).collect()
    } else {
        Vec::new()
    };
    let recoveries =
        OtelSignal::ALL.into_iter().filter(|signal| !current[signal.index()] && reported[signal.index()]).collect();
    (failures, recoveries)
}

fn otlp_signals(signals: &[OtelSignal]) -> String {
    signals.iter().map(|signal| signal.as_str()).collect::<Vec<_>>().join(",")
}

impl<S: Subscriber> Filter<S> for OtelErrorFilter {
    fn enabled(&self, meta: &Metadata<'_>, cx: &Context<'_, S>) -> bool {
        if self.reporter.is_some() && Self::is_otel_export_error(meta) {
            self.report_otel_export_failure(meta);
            return false;
        }
        if self.reporter.is_some() && Self::is_otel_export_success(meta) {
            self.report_otel_export_recovery(meta);
            return false;
        }
        <EnvFilter as Filter<S>>::enabled(&self.inner, meta, cx)
    }

    fn callsite_enabled(&self, meta: &'static Metadata<'static>) -> Interest {
        // Force per-call evaluation so export state transitions cannot bypass
        // the replacement messages through callsite caching.
        if self.reporter.is_some() && (Self::is_otel_export_error(meta) || Self::is_otel_export_success(meta)) {
            return Interest::sometimes();
        }
        <EnvFilter as Filter<S>>::callsite_enabled(&self.inner, meta)
    }

    fn max_level_hint(&self) -> Option<LevelFilter> {
        let hint = <EnvFilter as Filter<S>>::max_level_hint(&self.inner);
        if self.reporter.is_some() { hint.map(|level| level.max(LevelFilter::DEBUG)) } else { hint }
    }

    fn on_new_span(&self, attrs: &span::Attributes<'_>, id: &span::Id, ctx: Context<'_, S>) {
        <EnvFilter as Filter<S>>::on_new_span(&self.inner, attrs, id, ctx);
    }

    fn on_record(&self, id: &span::Id, values: &span::Record<'_>, ctx: Context<'_, S>) {
        <EnvFilter as Filter<S>>::on_record(&self.inner, id, values, ctx);
    }

    fn on_enter(&self, id: &span::Id, ctx: Context<'_, S>) {
        <EnvFilter as Filter<S>>::on_enter(&self.inner, id, ctx);
    }

    fn on_exit(&self, id: &span::Id, ctx: Context<'_, S>) {
        <EnvFilter as Filter<S>>::on_exit(&self.inner, id, ctx);
    }

    fn on_close(&self, id: span::Id, ctx: Context<'_, S>) {
        <EnvFilter as Filter<S>>::on_close(&self.inner, id, ctx);
    }
}

/// Resolve an environment filter with a fallback and a notice to emit after installing tracing.
pub fn filter_from_env(var: &str, default: &str) -> (EnvFilter, StartupNotice) {
    let (filter, value, provided_by_user, error) = match EnvFilter::try_from_env(var) {
        Ok(filter) => (filter, std::env::var(var).unwrap_or_default(), true, None),
        Err(e) => {
            let missing = matches!(e.source().and_then(|e| e.downcast_ref::<VarError>()), Some(VarError::NotPresent));
            #[expect(clippy::expect_used)]
            let filter = EnvFilter::try_new(default).expect("invalid default filter");
            (filter, default.to_string(), !missing, (!missing).then(|| e.to_string()))
        }
    };
    let var = var.to_string();
    let notice = Box::new(move || {
        if let Some(error) = error {
            warn!(setup::trace::FILTER, var, value, provided_by_user, provided_invalid = true, error);
        } else {
            info!(setup::trace::FILTER, var, value, provided_by_user);
        }
    });
    (filter, notice)
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering as AtomicOrdering},
    };

    use amaru_observability::tracing;
    use tracing_subscriber::prelude::*;

    use super::*;

    #[test]
    fn otel_export_error_is_recognised() {
        // Use the actual tracing machinery to produce `Metadata` with a known
        // target, level, and operation name, then classify it.
        assert_eq!(
            classify_emitted_event(|| {
                tracing::event!(
                    name: "BatchLogProcessor.ExportError",
                    target: "opentelemetry_sdk",
                    tracing::Level::ERROR,
                    "test"
                );
            }),
            Some(true)
        );
    }

    #[test]
    fn non_otel_target_is_not_recognised_as_export_error() {
        assert_eq!(
            classify_emitted_event(|| {
                tracing::event!(target: "amaru::stages", tracing::Level::DEBUG, "test");
            }),
            Some(false)
        );
    }

    #[test]
    fn otel_errors_are_suppressed_after_scheduling_one_warning() {
        let (reporter, transitions) = test_reporter();
        let filter = OtelErrorFilter::new(EnvFilter::try_new("error").unwrap(), Some(reporter));
        let seen = count_events(filter, || {
            tracing::event!(name: "BatchLogProcessor.ExportError", target: "opentelemetry_sdk", tracing::Level::ERROR, "first");
            tracing::event!(name: "BatchSpanProcessor.ExportError", target: "opentelemetry_sdk", tracing::Level::ERROR, "second");
        });
        assert_eq!(seen, 0);
        assert_eq!(transitions.recv().unwrap(), OtelExportTransition::Failed(OtelSignal::Logs));
        assert_eq!(transitions.recv().unwrap(), OtelExportTransition::Failed(OtelSignal::Traces));
    }

    #[test]
    fn successful_exports_batch_recovery_and_allow_a_later_failure_warning() {
        let available = [false; 3];
        let traces_and_logs_unavailable = [false, true, true];

        assert_eq!(
            otel_export_changes(&traces_and_logs_unavailable, &available),
            (vec![OtelSignal::Traces, OtelSignal::Logs], vec![])
        );
        assert_eq!(
            otel_export_changes(&available, &traces_and_logs_unavailable),
            (vec![], vec![OtelSignal::Traces, OtelSignal::Logs])
        );
        assert_eq!(otel_export_changes(&[false, false, true], &available), (vec![OtelSignal::Logs], vec![]));
    }

    #[test]
    fn non_error_otel_event_is_not_suppressed() {
        let filter = OtelErrorFilter::new(EnvFilter::try_new("debug").unwrap(), Some(test_reporter().0));
        let seen = count_events(filter, || {
            tracing::event!(name: "BatchLogProcessor.ExportError", target: "opentelemetry_sdk", tracing::Level::WARN, "test");
        });
        assert_eq!(seen, 1);
    }

    #[test]
    fn unrelated_otel_error_is_not_suppressed() {
        let filter = OtelErrorFilter::new(EnvFilter::try_new("error").unwrap(), Some(test_reporter().0));
        let seen = count_events(filter, || {
            tracing::event!(name: "TracerProvider.GlobalSetFailed", target: "opentelemetry", tracing::Level::ERROR, "test");
        });
        assert_eq!(seen, 1);
    }

    #[test]
    fn non_otel_error_is_not_suppressed() {
        let filter = OtelErrorFilter::new(EnvFilter::try_new("error").unwrap(), Some(test_reporter().0));
        let seen = count_events(filter, || {
            tracing::error!(target: "amaru::stages", "test");
        });
        assert_eq!(seen, 1);
    }

    // HELPERS

    fn test_reporter() -> (OtelExportReporter, mpsc::Receiver<OtelExportTransition>) {
        let (sender, receiver) = mpsc::channel();
        (OtelExportReporter { sender }, receiver)
    }

    /// Builds a subscriber that wraps an `OtelErrorFilter` and counts how
    /// many events pass the filter.  Installing it as the default inside a
    /// closure keeps tests independent even when run in parallel.
    struct CountingLayer {
        count: Arc<AtomicUsize>,
    }

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for CountingLayer {
        fn on_event(&self, _event: &tracing::Event<'_>, _ctx: tracing_subscriber::layer::Context<'_, S>) {
            self.count.fetch_add(1, AtomicOrdering::Relaxed);
        }
    }

    /// Classify the event actually dispatched by `emit`, not every callsite
    /// `enabled` during dispatcher rebuild (other tests register the same
    /// `BatchLogProcessor.ExportError` name at WARN).
    fn classify_emitted_event(emit: impl FnOnce()) -> Option<bool> {
        struct CaptureMeta(Arc<Mutex<Option<bool>>>);
        impl tracing::Subscriber for CaptureMeta {
            fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
                true
            }
            fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
                tracing::span::Id::from_u64(1)
            }
            fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
            fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
            fn event(&self, event: &tracing::Event<'_>) {
                *self.0.lock().unwrap() = Some(OtelErrorFilter::is_otel_export_error(event.metadata()));
            }
            fn enter(&self, _: &tracing::span::Id) {}
            fn exit(&self, _: &tracing::span::Id) {}
        }

        let check = Arc::new(Mutex::new(None));
        tracing::subscriber::with_default(CaptureMeta(Arc::clone(&check)), emit);
        *check.lock().unwrap()
    }

    /// Runs `f` with a subscriber that applies `filter` and returns the number
    /// of events that were seen by the inner layer.
    fn count_events<F: FnOnce()>(filter: OtelErrorFilter, f: F) -> usize {
        let count = Arc::new(AtomicUsize::new(0));
        let subscriber =
            tracing_subscriber::registry().with(CountingLayer { count: Arc::clone(&count) }.with_filter(filter));
        tracing::subscriber::with_default(subscriber, f);
        count.load(AtomicOrdering::Relaxed)
    }
}
