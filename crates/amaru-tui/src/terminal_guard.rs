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
    cell::Cell,
    io::{self, Stdout},
    sync::ReentrantLock,
};

use crossterm::{
    cursor, event, execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Frame, Terminal, backend::CrosstermBackend};

static TERMINAL_STATE: ReentrantLock<Cell<TerminalState>> = ReentrantLock::new(Cell::new(TerminalState::Inactive));

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TerminalState {
    Inactive,
    Active,
    Shutdown,
}

pub struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    mouse_capture_enabled: bool,
}

impl TerminalGuard {
    pub fn enter() -> io::Result<Self> {
        let state = TERMINAL_STATE.lock();
        match state.get() {
            TerminalState::Active => {
                return Err(io::Error::new(io::ErrorKind::AlreadyExists, "a terminal guard is already active"));
            }
            TerminalState::Shutdown => return Err(terminal_shutdown_error()),
            TerminalState::Inactive => {}
        }

        state.set(TerminalState::Active);
        let setup = || {
            enable_raw_mode()?;
            let mut stdout = std::io::stdout();
            execute!(stdout, EnterAlternateScreen, event::EnableMouseCapture)?;
            let backend = CrosstermBackend::new(stdout);
            let terminal = Terminal::new(backend)?;
            Ok(Self { terminal, mouse_capture_enabled: true })
        };
        match setup() {
            Ok(_) if state.get() == TerminalState::Shutdown => Err(terminal_shutdown_error()),
            Ok(terminal) => Ok(terminal),
            Err(error) => {
                restore_if_active();
                Err(error)
            }
        }
    }

    pub fn draw(&mut self, render: impl FnOnce(&mut Frame<'_>)) -> io::Result<()> {
        with_active(|| self.terminal.draw(render).map(|_| ()))
    }

    pub fn set_mouse_capture(&mut self, enabled: bool) -> io::Result<()> {
        with_active(|| {
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
        })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_if_active();
    }
}

pub fn emergency_restore_terminal() {
    let state = TERMINAL_STATE.lock();
    if state.replace(TerminalState::Shutdown) == TerminalState::Active {
        restore_terminal();
    }
}

fn with_active<T>(operation: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
    let state = TERMINAL_STATE.lock();
    match state.get() {
        TerminalState::Active => operation(),
        TerminalState::Inactive | TerminalState::Shutdown => Err(terminal_shutdown_error()),
    }
}

fn restore_if_active() {
    let state = TERMINAL_STATE.lock();
    if state.get() == TerminalState::Active {
        restore_terminal();
        if state.get() != TerminalState::Shutdown {
            state.set(TerminalState::Inactive);
        }
    }
}

fn restore_terminal() {
    let _ = disable_raw_mode();
    let mut stdout = std::io::stdout();
    let _ = execute!(stdout, event::DisableMouseCapture);
    let _ = execute!(stdout, LeaveAlternateScreen);
    let _ = execute!(stdout, cursor::Show);
}

fn terminal_shutdown_error() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "terminal lifecycle has shut down")
}
