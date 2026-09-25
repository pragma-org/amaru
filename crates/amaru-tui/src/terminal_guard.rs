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
    io::{self, Write},
    sync::atomic::{AtomicU8, Ordering},
};

use crossterm::{
    cursor, event, execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Frame, Terminal, backend::CrosstermBackend};

static TERMINAL_STATE: AtomicU8 = AtomicU8::new(TerminalState::Inactive as u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
enum TerminalState {
    Inactive,
    Active,
    Shutdown,
}

pub struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<TerminalOutput>>,
    mouse_capture_enabled: bool,
}

impl TerminalGuard {
    pub fn enter() -> io::Result<Self> {
        match TERMINAL_STATE.compare_exchange(
            TerminalState::Inactive as u8,
            TerminalState::Active as u8,
            Ordering::SeqCst,
            Ordering::SeqCst,
        ) {
            Ok(_) => {}
            Err(state) if state == TerminalState::Active as u8 => {
                return Err(io::Error::new(io::ErrorKind::AlreadyExists, "a terminal guard is already active"));
            }
            Err(_) => return Err(terminal_shutdown_error()),
        }

        let setup = || {
            enable_raw_mode()?;
            let mut output = TerminalOutput::default();
            execute!(output, EnterAlternateScreen, event::EnableMouseCapture)?;
            let backend = CrosstermBackend::new(output);
            let terminal = Terminal::new(backend)?;
            Ok(Self { terminal, mouse_capture_enabled: true })
        };
        match setup() {
            Ok(_) if TERMINAL_STATE.load(Ordering::SeqCst) == TerminalState::Shutdown as u8 => {
                let _ = disable_raw_mode();
                Err(terminal_shutdown_error())
            }
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

/// Stops dashboard output and restores the primary screen and terminal input before panic diagnostics are printed.
pub fn emergency_restore_terminal() {
    let was_active = {
        let mut stdout = io::stdout().lock();
        let was_active =
            TERMINAL_STATE.swap(TerminalState::Shutdown as u8, Ordering::SeqCst) == TerminalState::Active as u8;
        if was_active {
            restore_screen(&mut stdout);
        }
        was_active
    };
    if was_active {
        let _ = disable_raw_mode();
    }
}

fn with_active<T>(operation: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
    if TERMINAL_STATE.load(Ordering::SeqCst) == TerminalState::Active as u8 {
        operation()
    } else {
        Err(terminal_shutdown_error())
    }
}

fn restore_if_active() {
    {
        let mut stdout = io::stdout().lock();
        if TERMINAL_STATE.load(Ordering::SeqCst) != TerminalState::Active as u8 {
            return;
        }
        restore_screen(&mut stdout);
    }
    let _ = disable_raw_mode();
    let _ = TERMINAL_STATE.compare_exchange(
        TerminalState::Active as u8,
        TerminalState::Inactive as u8,
        Ordering::SeqCst,
        Ordering::SeqCst,
    );
}

fn restore_screen(stdout: &mut impl Write) {
    let _ = execute!(stdout, event::DisableMouseCapture);
    let _ = execute!(stdout, LeaveAlternateScreen);
    let _ = execute!(stdout, cursor::Show);
}

/// Buffers complete terminal commands so shutdown cannot interrupt an escape sequence.
/// Only flushing takes the stdout lock; rendering never holds it. After shutdown, buffered
/// output is discarded, including cursor restoration performed by Ratatui's destructor.
#[derive(Default)]
struct TerminalOutput {
    buffer: Vec<u8>,
}

impl Write for TerminalOutput {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buffer.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut stdout = io::stdout().lock();
        let result = if TERMINAL_STATE.load(Ordering::SeqCst) == TerminalState::Active as u8 {
            stdout.write_all(&self.buffer).and_then(|()| stdout.flush())
        } else {
            Ok(())
        };
        self.buffer.clear();
        result
    }
}

fn terminal_shutdown_error() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "terminal lifecycle has shut down")
}
