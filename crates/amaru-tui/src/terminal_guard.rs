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
use ratatui::{Frame, Terminal, TerminalOptions, Viewport, backend::CrosstermBackend, buffer::Buffer, layout::Rect};

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
    /// Area actually drawn. Narrower than the terminal when the screen has more cells than ratatui can address.
    viewport: Rect,
    /// Last frame `draw` put on screen. A text drag paints this back instead of rendering again.
    screen: Buffer,
}

/// `Buffer::pos_of` stores the cell index in a `u16`. Indexes `0..=65_535` are 65,536 cells.
/// One more cell and later rows are painted over earlier ones.
const MAX_TERMINAL_CELLS: u32 = u16::MAX as u32 + 1;

/// Keep the full height. Use only as many columns as fit in `MAX_TERMINAL_CELLS`.
pub(crate) fn fit_terminal_area(width: u16, height: u16) -> Rect {
    if width == 0 || height == 0 {
        return Rect::new(0, 0, width, height);
    }
    let cells = u32::from(width) * u32::from(height);
    if cells <= MAX_TERMINAL_CELLS {
        return Rect::new(0, 0, width, height);
    }
    let fitted_width = u16::try_from(MAX_TERMINAL_CELLS / u32::from(height)).unwrap_or(u16::MAX);
    Rect::new(0, 0, fitted_width.max(1), height)
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
            let size = ratatui::backend::Backend::size(&backend)?;
            let viewport = fit_terminal_area(size.width, size.height);
            let terminal = Terminal::with_options(backend, TerminalOptions { viewport: Viewport::Fixed(viewport) })?;
            Ok(Self { terminal, mouse_capture_enabled: true, viewport, screen: Buffer::default() })
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
        with_active(|| {
            self.sync_viewport()?;
            let completed = self.terminal.draw(render)?;
            self.screen = completed.buffer.clone();
            Ok(())
        })
    }

    /// The frame last painted by [`Self::draw`].
    pub fn snapshot_screen(&self) -> Buffer {
        self.screen.clone()
    }

    /// Paint `snapshot` plus a small overlay. Does not replace [`Self::snapshot_screen`].
    pub fn draw_buffer(&mut self, snapshot: &Buffer, paint: impl FnOnce(&mut Buffer)) -> io::Result<()> {
        with_active(|| {
            self.sync_viewport()?;
            self.terminal.draw(|frame| {
                copy_cells(frame.buffer_mut(), snapshot);
                paint(frame.buffer_mut());
            })?;
            Ok(())
        })
    }

    fn sync_viewport(&mut self) -> io::Result<()> {
        let size = self.terminal.size()?;
        let area = fit_terminal_area(size.width, size.height);
        if area != self.viewport {
            self.terminal.resize(area)?;
            self.viewport = area;
        }
        Ok(())
    }

    /// Write bytes that are not a ratatui frame, such as an OSC 52 clipboard sequence.
    pub fn write_raw(&mut self, bytes: &[u8]) -> io::Result<()> {
        with_active(|| {
            let backend = self.terminal.backend_mut();
            backend.write_all(bytes)?;
            backend.flush()
        })
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

fn copy_cells(target: &mut Buffer, source: &Buffer) {
    let width = target.area.width.min(source.area.width);
    let height = target.area.height.min(source.area.height);
    for y in 0..height {
        for x in 0..width {
            let Some(cell) = source.cell((source.area.x.saturating_add(x), source.area.y.saturating_add(y))) else {
                continue;
            };
            let Some(slot) = target.cell_mut((target.area.x.saturating_add(x), target.area.y.saturating_add(y))) else {
                continue;
            };
            *slot = cell.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use ratatui::{Terminal, TerminalOptions, Viewport, backend::TestBackend, layout::Rect, widgets::Paragraph};

    use super::{MAX_TERMINAL_CELLS, fit_terminal_area};

    fn marked_corner(width: u16, height: u16) -> (u16, u16) {
        let area = Rect::new(0, 0, width, height);
        let mut marked = ratatui::buffer::Buffer::empty(area);
        marked[(width - 1, height - 1)].set_symbol("Z");
        let blank = ratatui::buffer::Buffer::empty(area);
        blank
            .diff(&marked)
            .into_iter()
            .find(|(_, _, cell)| cell.symbol() == "Z")
            .map(|(x, y, _)| (x, y))
            .expect("corner")
    }

    #[test]
    fn ratatui_addresses_65536_cells_and_not_one_more() {
        assert_eq!(marked_corner(256, 256), (255, 255));
        assert_ne!(marked_corner(256, 257), (255, 256));
    }

    #[test]
    fn fit_keeps_a_normal_terminal_and_narrows_a_large_one() {
        assert_eq!(fit_terminal_area(80, 24), Rect::new(0, 0, 80, 24));
        assert_eq!(fit_terminal_area(0, 40), Rect::new(0, 0, 0, 40));

        let wide = fit_terminal_area(400, 200);
        assert_eq!(wide.height, 200);
        assert_eq!(wide.width, 327);
        assert!(u32::from(wide.width) * u32::from(wide.height) <= MAX_TERMINAL_CELLS);
        assert!(u32::from(wide.width + 1) * u32::from(wide.height) > MAX_TERMINAL_CELLS);

        let tall = fit_terminal_area(2, 65_535);
        assert_eq!(tall, Rect::new(0, 0, 1, 65_535));
        assert!(u32::from(tall.width) * u32::from(tall.height) <= MAX_TERMINAL_CELLS);
    }

    #[test]
    fn a_wide_terminal_draws_full_height_and_leaves_the_extra_columns_blank() {
        let backend = TestBackend::new(400, 200);
        let mut terminal =
            Terminal::with_options(backend, TerminalOptions { viewport: Viewport::Fixed(Rect::new(0, 0, 1, 1)) })
                .expect("terminal");
        let size = terminal.size().expect("size");
        let area = fit_terminal_area(size.width, size.height);
        terminal.resize(area).expect("resize");

        terminal
            .draw(|frame| {
                assert_eq!(frame.area(), area);
                let line = "X".repeat(frame.area().width as usize);
                frame.render_widget(Paragraph::new(line), Rect::new(0, 0, frame.area().width, 1));
            })
            .expect("draw");

        let buffer = terminal.backend().buffer();
        assert_eq!(buffer.cell((326, 0)).expect("last used column").symbol(), "X");
        assert_eq!(buffer.cell((327, 0)).expect("first unused column").symbol(), " ");
        assert_eq!(buffer.cell((399, 199)).expect("bottom right").symbol(), " ");
    }
}
