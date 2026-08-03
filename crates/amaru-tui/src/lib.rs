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
    io::{self, IsTerminal, Stdout},
    sync::{
        Arc,
        mpsc::{self, Receiver, Sender},
    },
    thread::{self, JoinHandle},
    time::Instant,
};

use amaru_metrics::MetricsSubscriber;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, MouseButton, MouseEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend, layout::Rect};

mod capture;
mod config;
mod events;
mod metrics;
mod model;
mod settings;
mod startup;
mod ui;

pub use capture::TracingLayer;
pub use config::{Config, TimeWindow, format_windows};
use events::Message;
use metrics::MetricsLayer;
use model::Model;
pub use model::{InteractionMode, LevelFilter, Page, PaneMode, ScrollFocus, TargetFilter};
pub use settings::Settings;
pub use startup::{ConfigEntry, ConfigSection, ProcessInfo, RuntimeSettingsSource, StartupContext};
use ui::Hotspots;

pub struct Session {
    layer: TracingLayer,
    metrics: Arc<MetricsLayer>,
    control_tx: Sender<Control>,
    join: Option<JoinHandle<io::Result<()>>>,
}

impl Session {
    pub fn spawn(config: Config, startup: StartupContext) -> io::Result<Self> {
        let (telemetry_tx, telemetry_rx) = mpsc::sync_channel(config.channel_capacity);
        let (control_tx, control_rx) = mpsc::channel();
        let layer = TracingLayer::new(telemetry_tx);
        let metrics = Arc::new(MetricsLayer::new(layer.sender()));

        let join = thread::Builder::new()
            .name("amaru-tui".into())
            .spawn(move || run_terminal(config, startup, telemetry_rx, control_rx))
            .map_err(|err| io::Error::other(format!("failed to spawn tui thread: {err}")))?;

        Ok(Self { layer, metrics, control_tx, join: Some(join) })
    }

    pub fn layer(&self) -> TracingLayer {
        self.layer.clone()
    }

    pub fn metrics_subscriber(&self) -> Arc<dyn MetricsSubscriber> {
        self.metrics.clone()
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
    telemetry_rx: Receiver<Message>,
    control_rx: Receiver<Control>,
) -> io::Result<()> {
    let mut terminal = TerminalGuard::enter()?;
    let mut model = Model::new(config.clone(), startup);
    let mut hotspots = Hotspots::default();

    loop {
        if control_rx.try_recv().is_ok() {
            return Ok(());
        }

        while let Ok(message) = telemetry_rx.try_recv() {
            model.handle_message(message);
        }
        let now = Instant::now();

        if !model.is_copy_mode() {
            terminal.terminal.draw(|frame| ui::render(frame, &model, &mut hotspots, now))?;
        }

        let timeout = config.tick_interval;
        if !event::poll(timeout)? {
            continue;
        }

        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                if model.is_copy_mode() {
                    if key.code == KeyCode::Esc {
                        terminal.set_mouse_capture(true)?;
                        model.exit_copy_mode();
                    }
                    continue;
                }

                match key.code {
                    KeyCode::Esc => enter_copy_mode(&mut terminal, &mut model, &mut hotspots, now)?,
                    KeyCode::Char('q') => request_shutdown()?,
                    KeyCode::Char('c') if key.modifiers.contains(event::KeyModifiers::CONTROL) => request_shutdown()?,
                    KeyCode::Tab | KeyCode::Right => model.next_page(),
                    KeyCode::BackTab => model.previous_page(),
                    KeyCode::Left => model.previous_page(),
                    KeyCode::Char('f') => model.next_scroll_focus(),
                    KeyCode::Char('+') | KeyCode::Char('=') => match (model.page, model.scroll_focus) {
                        (model::Page::Cardano, ScrollFocus::Proposals) => model.cycle_proposal_pane(),
                        (model::Page::Amaru, ScrollFocus::Peers) => model.cycle_peer_pane(),
                        _ => model.cycle_log_pane(),
                    },
                    KeyCode::Up => model.scroll_focused(-1),
                    KeyCode::Down => model.scroll_focused(1),
                    KeyCode::PageUp => model.scroll_focused(-10),
                    KeyCode::PageDown => model.scroll_focused(10),
                    KeyCode::Backspace
                    | KeyCode::Enter
                    | KeyCode::Home
                    | KeyCode::End
                    | KeyCode::Delete
                    | KeyCode::Insert
                    | KeyCode::F(_)
                    | KeyCode::Char(_)
                    | KeyCode::Null
                    | KeyCode::CapsLock
                    | KeyCode::ScrollLock
                    | KeyCode::NumLock
                    | KeyCode::PrintScreen
                    | KeyCode::Pause
                    | KeyCode::Menu
                    | KeyCode::KeypadBegin
                    | KeyCode::Media(_)
                    | KeyCode::Modifier(_) => {}
                }
            }
            Event::Mouse(mouse) => match mouse.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    let point = Rect { x: mouse.column, y: mouse.row, width: 1, height: 1 };
                    handle_click(&mut model, &hotspots, point);
                }
                MouseEventKind::ScrollDown => {
                    let point = Rect { x: mouse.column, y: mouse.row, width: 1, height: 1 };
                    handle_scroll(&mut model, &hotspots, point, 3);
                }
                MouseEventKind::ScrollUp => {
                    let point = Rect { x: mouse.column, y: mouse.row, width: 1, height: 1 };
                    handle_scroll(&mut model, &hotspots, point, -3);
                }
                MouseEventKind::Down(_)
                | MouseEventKind::Up(_)
                | MouseEventKind::Drag(_)
                | MouseEventKind::Moved
                | MouseEventKind::ScrollLeft
                | MouseEventKind::ScrollRight => {}
            },
            Event::Resize(_, _) => {}
            Event::FocusGained | Event::FocusLost | Event::Paste(_) | Event::Key(_) => {}
        }
    }
}

fn handle_click(model: &mut Model, hotspots: &Hotspots, point: Rect) {
    for (page, rect) in &hotspots.page_tabs {
        if intersects(*rect, point) {
            model.set_page(*page);
            return;
        }
    }
    if intersects(hotspots.log_toggle, point) {
        model.cycle_log_pane();
        return;
    }
    if intersects(hotspots.peer_toggle, point) {
        model.cycle_peer_pane();
        return;
    }
    if intersects(hotspots.proposal_toggle, point) {
        model.cycle_proposal_pane();
        return;
    }
    if intersects(hotspots.logs_area, point) {
        model.focus_logs();
    } else if intersects(hotspots.peers_area, point) {
        model.focus_peers();
    } else if intersects(hotspots.proposals_area, point) {
        model.focus_proposals();
    }

    for (index, rect) in hotspots.window_tabs.iter().enumerate() {
        if intersects(*rect, point) {
            model.set_window(index);
            return;
        }
    }

    for (filter, rect) in &hotspots.level_tabs {
        if intersects(*rect, point) {
            model.set_level_filter(*filter);
            return;
        }
    }

    for (filter, rect) in &hotspots.target_tabs {
        if intersects(*rect, point) {
            model.set_target_filter(*filter);
            return;
        }
    }
}

fn handle_scroll(model: &mut Model, hotspots: &Hotspots, point: Rect, delta: isize) {
    if intersects(hotspots.peers_area, point) {
        model.scroll_peers(delta);
        return;
    }

    if intersects(hotspots.proposals_area, point) {
        model.scroll_proposals(delta);
        return;
    }

    if intersects(hotspots.logs_area, point) {
        model.scroll_logs(delta);
        return;
    }

    model.scroll_logs(delta);
}

fn enter_copy_mode(
    terminal: &mut TerminalGuard,
    model: &mut Model,
    hotspots: &mut Hotspots,
    now: Instant,
) -> io::Result<()> {
    model.enter_copy_mode();
    terminal.terminal.draw(|frame| ui::render(frame, model, hotspots, now))?;
    terminal.set_mouse_capture(false)
}

fn intersects(a: Rect, b: Rect) -> bool {
    b.x >= a.x && b.x < a.x + a.width && b.y >= a.y && b.y < a.y + a.height
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

struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    mouse_capture_enabled: bool,
}

impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = std::io::stdout();
        execute!(stdout, EnterAlternateScreen, event::EnableMouseCapture)?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;
        Ok(Self { terminal, mouse_capture_enabled: true })
    }

    fn set_mouse_capture(&mut self, enabled: bool) -> io::Result<()> {
        if self.mouse_capture_enabled == enabled {
            return Ok(());
        }

        if enabled {
            execute!(self.terminal.backend_mut(), event::EnableMouseCapture)?;
        } else {
            execute!(self.terminal.backend_mut(), event::DisableMouseCapture)?;
        }

        self.mouse_capture_enabled = enabled;
        Ok(())
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = self.set_mouse_capture(false);
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
        let _ = self.terminal.show_cursor();
    }
}
