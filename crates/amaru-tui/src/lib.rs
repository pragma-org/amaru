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
    mem::MaybeUninit,
    sync::mpsc::{self, Receiver, Sender},
    thread::{self, JoinHandle},
    time::Instant,
};

use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, MouseButton, MouseEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend, layout::Rect};
use sysinfo::{CpuRefreshKind, MemoryRefreshKind, ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};

mod capture;
mod config;
mod events;
mod model;
mod startup;
mod ui;

pub use capture::TracingLayer;
pub use config::{Config, parse_windows};
use events::{Message, SystemSample};
use model::Model;
pub use model::{LevelFilter, LogPaneMode, Page, TargetFilter};
pub use startup::{ConfigEntry, ConfigSection, ProcessInfo, StartupContext};
use ui::Hotspots;

pub struct Session {
    layer: TracingLayer,
    control_tx: Sender<Control>,
    join: Option<JoinHandle<io::Result<()>>>,
}

impl Session {
    pub fn spawn(config: Config, startup: StartupContext) -> io::Result<Self> {
        let (telemetry_tx, telemetry_rx) = mpsc::sync_channel(config.channel_capacity);
        let (control_tx, control_rx) = mpsc::channel();
        let layer = TracingLayer::new(telemetry_tx);

        let join = thread::Builder::new()
            .name("amaru-tui".into())
            .spawn(move || run_terminal(config, startup, telemetry_rx, control_rx))
            .map_err(|err| io::Error::other(format!("failed to spawn tui thread: {err}")))?;

        Ok(Self { layer, control_tx, join: Some(join) })
    }

    pub fn layer(&self) -> TracingLayer {
        self.layer.clone()
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
    let mut system = System::new_with_specifics(
        RefreshKind::nothing()
            .with_cpu(CpuRefreshKind::everything().without_frequency())
            .with_memory(MemoryRefreshKind::everything().without_swap()),
    );
    let own_pid = sysinfo::get_current_pid().ok();
    let mut last_sample = Instant::now().checked_sub(config.sample_interval).unwrap_or_else(Instant::now);

    loop {
        if control_rx.try_recv().is_ok() {
            return Ok(());
        }

        while let Ok(message) = telemetry_rx.try_recv() {
            model.handle_message(message);
        }

        let now = Instant::now();
        if now.duration_since(last_sample) >= config.sample_interval {
            if let Some(sample) = collect_sample(&mut system, own_pid) {
                model.push_system_sample(sample);
            }
            last_sample = now;
        }

        terminal.terminal.draw(|frame| ui::render(frame, &model, &mut hotspots, now))?;

        let timeout = config.tick_interval;
        if !event::poll(timeout)? {
            continue;
        }

        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                KeyCode::Char('q') => request_shutdown()?,
                KeyCode::Char('c') if key.modifiers.contains(event::KeyModifiers::CONTROL) => request_shutdown()?,
                KeyCode::Tab | KeyCode::Right => model.next_page(),
                KeyCode::BackTab => model.previous_page(),
                KeyCode::Left => model.previous_page(),
                KeyCode::Char('+') | KeyCode::Char('=') => model.cycle_log_pane(),
                KeyCode::Up => model.scroll_logs(-1),
                KeyCode::Down => model.scroll_logs(1),
                KeyCode::PageUp => model.scroll_logs(-(model.logs.len().min(10) as isize)),
                KeyCode::PageDown => model.scroll_logs(model.logs.len().min(10) as isize),
                KeyCode::Backspace
                | KeyCode::Enter
                | KeyCode::Home
                | KeyCode::End
                | KeyCode::Delete
                | KeyCode::Insert
                | KeyCode::F(_)
                | KeyCode::Char(_)
                | KeyCode::Null
                | KeyCode::Esc
                | KeyCode::CapsLock
                | KeyCode::ScrollLock
                | KeyCode::NumLock
                | KeyCode::PrintScreen
                | KeyCode::Pause
                | KeyCode::Menu
                | KeyCode::KeypadBegin
                | KeyCode::Media(_)
                | KeyCode::Modifier(_) => {}
            },
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

fn intersects(a: Rect, b: Rect) -> bool {
    b.x >= a.x && b.x < a.x + a.width && b.y >= a.y && b.y < a.y + a.height
}

fn collect_sample(system: &mut System, own_pid: Option<sysinfo::Pid>) -> Option<SystemSample> {
    let pid = own_pid?;
    system.refresh_memory();
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::nothing().with_cpu().with_disk_usage().with_memory(),
    );
    let process = system.process(pid)?;
    let disk = process.disk_usage();

    Some(SystemSample {
        at: Instant::now(),
        cpu_percent: process.cpu_usage() as f64,
        process_memory_bytes: process_memory_bytes(pid, process.memory()),
        rss_bytes: process.memory(),
        virtual_bytes: process.virtual_memory(),
        memory_used_bytes: system.used_memory(),
        memory_total_bytes: system.total_memory(),
        disk_read_bytes: disk.total_read_bytes,
        disk_write_bytes: disk.total_written_bytes,
    })
}

#[cfg(target_os = "macos")]
fn process_memory_bytes(pid: sysinfo::Pid, fallback: u64) -> u64 {
    let mut info = MaybeUninit::<libc::rusage_info_v4>::zeroed();
    let result =
        unsafe { libc::proc_pid_rusage(pid.as_u32() as libc::c_int, libc::RUSAGE_INFO_V4, info.as_mut_ptr().cast()) };

    if result == 0 { unsafe { info.assume_init().ri_phys_footprint } } else { fallback }
}

#[cfg(not(target_os = "macos"))]
fn process_memory_bytes(_pid: sysinfo::Pid, fallback: u64) -> u64 {
    fallback
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
}

impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = std::io::stdout();
        execute!(stdout, EnterAlternateScreen, event::EnableMouseCapture)?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;
        Ok(Self { terminal })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen, event::DisableMouseCapture);
        let _ = self.terminal.show_cursor();
    }
}
