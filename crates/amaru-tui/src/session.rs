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

use std::{
    io::{self, IsTerminal},
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use amaru_metrics::MetricsEvent;
use crossterm::event;

use crate::{
    Config, TracingLayer,
    events::{Message, MetricRecord},
    model::{Model, TerminalEventOutcome},
    startup::StartupContext,
    terminal_guard::TerminalGuard,
    ui::{self, Views},
};

pub struct Session {
    tx: SyncSender<crate::events::Message>,
    control_tx: Sender<Control>,
    join: Option<JoinHandle<io::Result<()>>>,
}

impl Session {
    pub fn spawn(config: Config, startup: StartupContext, signal_count: Arc<AtomicU8>) -> io::Result<Self> {
        let (telemetry_tx, telemetry_rx) = mpsc::sync_channel(config.channel_capacity);
        let (control_tx, control_rx) = mpsc::channel();

        let join = thread::Builder::new()
            .name("amaru-tui".into())
            .spawn(move || run_terminal(config, startup, telemetry_rx, control_rx, signal_count))
            .map_err(|err| io::Error::other(format!("failed to spawn tui thread: {err}")))?;

        Ok(Self { tx: telemetry_tx, control_tx, join: Some(join) })
    }

    pub fn tracing_layer(&self) -> TracingLayer {
        TracingLayer::new(self.tx.clone())
    }

    pub fn metrics_observer(&self) -> Box<dyn Fn(&MetricsEvent) + Send + Sync> {
        let tx = self.tx.clone();
        Box::new(move |event| {
            let _ = tx.try_send(Message::Metrics(MetricRecord { at: Instant::now(), event: event.clone() }));
        })
    }

    pub fn shutdown(mut self) -> io::Result<()> {
        self.shutdown_inner()
    }

    fn shutdown_inner(&mut self) -> io::Result<()> {
        let _ = self.control_tx.send(Control::Shutdown);
        if let Some(join) = self.join.take() {
            join.join().map_err(|_| io::Error::other("tui thread panicked"))?
        } else {
            Ok(())
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.shutdown_inner();
    }
}

pub fn should_enable(no_tui: bool, with_json_traces: bool) -> bool {
    if no_tui || with_json_traces || !cfg!(unix) {
        return false;
    }

    if std::env::var("TERM").is_ok_and(|value| value == "dumb") {
        return false;
    }

    std::io::stdout().is_terminal()
}

enum Control {
    Shutdown,
}

fn run_terminal(
    config: Config,
    startup: StartupContext,
    telemetry_rx: Receiver<crate::events::Message>,
    control_rx: Receiver<Control>,
    signal_count: Arc<AtomicU8>,
) -> io::Result<()> {
    let mut terminal = TerminalGuard::enter()?;
    let mut model = Model::new(config.clone(), startup);
    let mut views = Views::default();
    let mut next_draw_at = Instant::now();

    loop {
        if control_rx.try_recv().is_ok() {
            return Ok(());
        }

        if signal_count.load(Ordering::SeqCst) > 0 && !model.is_shutdown_mode() {
            model.enter_shutdown_mode();
            terminal.set_mouse_capture(true)?;
        }

        let now = Instant::now();
        if now >= next_draw_at {
            if !model.is_copy_mode() {
                terminal.terminal().draw(|frame| ui::render(frame, &model, &mut views, now))?;
            }
            next_draw_at = now + config.tick_interval;
        }

        while event::poll(Duration::ZERO)? {
            match model.handle_terminal_event(event::read()?, &views) {
                TerminalEventOutcome::Continue => {}
                TerminalEventOutcome::EnterCopyMode => enter_copy_mode(&mut terminal, &model, &mut views, now)?,
                TerminalEventOutcome::ExitCopyMode => terminal.set_mouse_capture(true)?,
                TerminalEventOutcome::Shutdown => request_shutdown()?,
            }
        }

        let timeout = next_draw_at.saturating_duration_since(Instant::now());
        match telemetry_rx.recv_timeout(timeout) {
            Ok(message) => {
                model.handle_message(message);
                while let Ok(message) = telemetry_rx.try_recv() {
                    model.handle_message(message);
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {}
        }
    }
}

fn enter_copy_mode(terminal: &mut TerminalGuard, model: &Model, views: &mut Views, now: Instant) -> io::Result<()> {
    terminal.terminal().draw(|frame| ui::render(frame, model, views, now))?;
    terminal.set_mouse_capture(false)
}

#[cfg(unix)]
fn request_shutdown() -> io::Result<()> {
    signal_hook::low_level::raise(signal_hook::consts::SIGINT)
        .map_err(|err| io::Error::other(format!("failed to raise SIGINT: {err}")))
}

#[cfg(not(unix))]
fn request_shutdown() -> io::Result<()> {
    Ok(())
}
