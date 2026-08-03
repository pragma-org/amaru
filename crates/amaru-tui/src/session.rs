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
        mpsc::{self, Receiver, Sender},
    },
    thread::{self, JoinHandle},
    time::Instant,
};

use crossterm::event;

use crate::{
    Config, TracingLayer, metrics,
    model::{Model, TerminalEventOutcome},
    startup::StartupContext,
    terminal_guard::TerminalGuard,
    ui::{self, Views},
};

pub struct Session {
    layer: TracingLayer,
    metrics: Arc<metrics::Subscriber>,
    control_tx: Sender<Control>,
    join: Option<JoinHandle<io::Result<()>>>,
}

impl Session {
    pub fn spawn(config: Config, startup: StartupContext) -> io::Result<Self> {
        let (telemetry_tx, telemetry_rx) = mpsc::sync_channel(config.channel_capacity);
        let (control_tx, control_rx) = mpsc::channel();
        let layer = TracingLayer::new(telemetry_tx);
        let metrics = Arc::new(metrics::Subscriber::new(layer.sender()));

        let join = thread::Builder::new()
            .name("amaru-tui".into())
            .spawn(move || run_terminal(config, startup, telemetry_rx, control_rx))
            .map_err(|err| io::Error::other(format!("failed to spawn tui thread: {err}")))?;

        Ok(Self { layer, metrics, control_tx, join: Some(join) })
    }

    pub fn layer(&self) -> TracingLayer {
        self.layer.clone()
    }

    pub fn subscribe_to_metrics(&self) -> metrics::Subscription {
        metrics::Subscription::new(self.metrics.clone())
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
) -> io::Result<()> {
    let mut terminal = TerminalGuard::enter()?;
    let mut model = Model::new(config.clone(), startup);
    let mut views = Views::default();

    loop {
        if control_rx.try_recv().is_ok() {
            return Ok(());
        }

        while let Ok(message) = telemetry_rx.try_recv() {
            model.handle_message(message);
        }

        let now = Instant::now();
        if !model.is_copy_mode() {
            terminal.terminal().draw(|frame| ui::render(frame, &model, &mut views, now))?;
        }

        if !event::poll(config.tick_interval)? {
            continue;
        }

        match model.handle_terminal_event(event::read()?, &views) {
            TerminalEventOutcome::Continue => {}
            TerminalEventOutcome::EnterCopyMode => enter_copy_mode(&mut terminal, &mut model, &mut views, now)?,
            TerminalEventOutcome::ExitCopyMode => terminal.set_mouse_capture(true)?,
            TerminalEventOutcome::Shutdown => request_shutdown()?,
        }
    }
}

fn enter_copy_mode(terminal: &mut TerminalGuard, model: &mut Model, views: &mut Views, now: Instant) -> io::Result<()> {
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
