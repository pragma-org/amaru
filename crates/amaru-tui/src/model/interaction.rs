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

use std::rc::Rc;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::layout::Rect;
use regex::Regex;

use super::{
    prompt::{PromptAction, PromptKind, PromptState},
    *,
};
use crate::{events::TelemetryRecord, ui::Views};

impl Model {
    pub fn handle_terminal_event(&mut self, event: Event, views: &Views) -> TerminalEventOutcome {
        if views.logs_area.height > 0 {
            self.logs_viewport_rows = (views.logs_area.height as usize).saturating_sub(6).max(1);
        }

        match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => self.handle_key_event(key),
            Event::Mouse(mouse) => self.handle_mouse_event(mouse, views),
            Event::Resize(_, _) => TerminalEventOutcome::Continue,
            Event::FocusGained | Event::FocusLost | Event::Paste(_) | Event::Key(_) => TerminalEventOutcome::Continue,
        }
    }

    pub fn sync_logs(&mut self) {
        self.logs.sync(self.level_filter, self.target_filter, &self.text_filter_pattern, self.text_filter.as_ref());
    }

    pub fn next_page(&mut self) {
        self.set_page(self.page.next());
    }

    pub fn previous_page(&mut self) {
        self.set_page(self.page.previous());
    }

    pub fn set_page(&mut self, page: Page) {
        self.page = page;
        self.scroll_focus = match self.page {
            Page::Amaru if matches!(self.scroll_focus, ScrollFocus::Logs | ScrollFocus::Peers) => self.scroll_focus,
            Page::Cardano if matches!(self.scroll_focus, ScrollFocus::Logs | ScrollFocus::Proposals) => {
                self.scroll_focus
            }
            Page::Config => ScrollFocus::Config,
            Page::Amaru | Page::Cardano => ScrollFocus::Logs,
        };
    }

    pub fn enter_copy_mode(&mut self) {
        self.interaction_mode = InteractionMode::Copy;
    }

    pub fn enter_shutdown_mode(&mut self) {
        self.interaction_mode = InteractionMode::Shutdown;
    }

    pub fn exit_copy_mode(&mut self) {
        self.interaction_mode = InteractionMode::Normal;
    }

    pub fn is_copy_mode(&self) -> bool {
        self.interaction_mode == InteractionMode::Copy
    }

    pub fn is_shutdown_mode(&self) -> bool {
        self.interaction_mode == InteractionMode::Shutdown
    }

    pub fn cycle_log_pane(&mut self) {
        self.log_pane_mode = self.log_pane_mode.toggle();
        if self.log_pane_mode.is_maximized() {
            self.peer_pane_mode = PaneMode::Normal;
            self.proposal_pane_mode = PaneMode::Normal;
        }
    }

    pub fn cycle_peer_pane(&mut self) {
        self.peer_pane_mode = self.peer_pane_mode.toggle();
        if self.peer_pane_mode.is_maximized() {
            self.log_pane_mode = PaneMode::Normal;
            self.proposal_pane_mode = PaneMode::Normal;
        }
    }

    pub fn cycle_proposal_pane(&mut self) {
        self.proposal_pane_mode = self.proposal_pane_mode.toggle();
        if self.proposal_pane_mode.is_maximized() {
            self.log_pane_mode = PaneMode::Normal;
            self.peer_pane_mode = PaneMode::Normal;
        }
    }

    pub fn next_scroll_focus(&mut self) {
        self.scroll_focus = self.scroll_focus.next_for(self.page);
    }

    pub fn previous_scroll_focus(&mut self) {
        self.scroll_focus = self.scroll_focus.previous_for(self.page);
    }

    pub fn set_level_filter(&mut self, level: LevelFilter) {
        self.level_filter = level;
        self.sync_logs();
        self.log_scroll = 0;
        self.scroll_focus = ScrollFocus::Logs;
    }

    pub fn set_target_filter(&mut self, filter: TargetFilter) {
        self.target_filter = filter;
        self.sync_logs();
        self.log_scroll = 0;
        self.scroll_focus = ScrollFocus::Logs;
    }

    pub fn scroll_focused(&mut self, delta: isize) {
        match self.scroll_focus {
            ScrollFocus::Logs if self.highlight.is_some() && delta.abs() == 1 => self.jump_highlight(delta),
            ScrollFocus::Logs => self.scroll_logs(delta),
            ScrollFocus::Peers => self.scroll_peers(delta),
            ScrollFocus::Proposals => self.scroll_proposals(delta),
            ScrollFocus::Config => self.scroll_config(delta),
        }
    }

    pub fn scroll_logs(&mut self, delta: isize) {
        if delta.is_negative() {
            self.log_scroll = self.log_scroll.saturating_sub(delta.unsigned_abs());
        } else {
            self.log_scroll = self.log_scroll.saturating_add(delta as usize);
        }
    }

    pub fn scroll_peers(&mut self, delta: isize) {
        if delta.is_negative() {
            self.peer_scroll = self.peer_scroll.saturating_sub(delta.unsigned_abs());
        } else {
            self.peer_scroll = self.peer_scroll.saturating_add(delta as usize);
        }
    }

    pub fn scroll_proposals(&mut self, delta: isize) {
        if delta.is_negative() {
            self.proposal_scroll = self.proposal_scroll.saturating_sub(delta.unsigned_abs());
        } else {
            self.proposal_scroll = self.proposal_scroll.saturating_add(delta as usize);
        }
    }

    pub fn scroll_config(&mut self, delta: isize) {
        if delta.is_negative() {
            self.config_scroll = self.config_scroll.saturating_sub(delta.unsigned_abs());
        } else {
            self.config_scroll = self.config_scroll.saturating_add(delta as usize);
        }
    }

    pub fn handle_click(&mut self, views: &Views, point: Rect) {
        if let Some(page) = views.page_at(point) {
            self.set_page(page);
            return;
        }

        if views.toggles_logs(point) {
            self.cycle_log_pane();
            return;
        }

        if views.toggles_peers(point) {
            self.cycle_peer_pane();
            return;
        }

        if views.toggles_proposals(point) {
            self.cycle_proposal_pane();
            return;
        }

        if let Some(focus) = views.focus_at(point) {
            self.set_scroll_focus(focus);
        }

        if let Some(level) = views.level_filter_at(point) {
            self.set_level_filter(level);
            return;
        }

        if let Some(filter) = views.target_filter_at(point) {
            self.set_target_filter(filter);
        }
    }

    pub fn handle_scroll(&mut self, views: &Views, point: Rect, delta: isize) {
        self.set_scroll_focus(views.scroll_focus_at(point));
        self.scroll_focused(delta);
    }

    pub fn toggle_focused_pane(&mut self) -> bool {
        match (self.page, self.scroll_focus) {
            (Page::Amaru | Page::Cardano, ScrollFocus::Logs) => {
                self.cycle_log_pane();
                true
            }
            (Page::Amaru, ScrollFocus::Peers) => {
                self.cycle_peer_pane();
                true
            }
            (Page::Cardano, ScrollFocus::Proposals) => {
                self.cycle_proposal_pane();
                true
            }
            (Page::Amaru, ScrollFocus::Proposals | ScrollFocus::Config)
            | (Page::Cardano, ScrollFocus::Peers | ScrollFocus::Config)
            | (Page::Config, _) => false,
        }
    }

    pub(super) fn handle_key_event(&mut self, key: event::KeyEvent) -> TerminalEventOutcome {
        if self.is_shutdown_mode() {
            return TerminalEventOutcome::Continue;
        }

        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return TerminalEventOutcome::Shutdown;
        }

        if self.prompt.is_some() {
            return self.handle_prompt_key(key);
        }

        if self.is_copy_mode() {
            return self.handle_copy_mode_key(key);
        }

        match key.code {
            KeyCode::Esc => {
                if !self.is_ready(std::time::Instant::now()) {
                    return TerminalEventOutcome::Continue;
                }
                self.enter_copy_mode();
                TerminalEventOutcome::EnterCopyMode
            }
            KeyCode::Char('q') => TerminalEventOutcome::Shutdown,
            KeyCode::Char('&') => {
                self.open_prompt(PromptKind::Filter);
                TerminalEventOutcome::Continue
            }
            KeyCode::Char('/') => {
                self.open_prompt(PromptKind::Highlight);
                TerminalEventOutcome::Continue
            }
            KeyCode::Tab => {
                self.next_page();
                TerminalEventOutcome::Continue
            }
            KeyCode::BackTab => {
                self.previous_page();
                TerminalEventOutcome::Continue
            }
            KeyCode::Right => {
                self.next_scroll_focus();
                TerminalEventOutcome::Continue
            }
            KeyCode::Left => {
                self.previous_scroll_focus();
                TerminalEventOutcome::Continue
            }
            KeyCode::Enter => {
                let _ = self.toggle_focused_pane();
                TerminalEventOutcome::Continue
            }
            KeyCode::Char('+') | KeyCode::Char('=') => {
                let _ = self.toggle_focused_pane();
                TerminalEventOutcome::Continue
            }
            KeyCode::Up => {
                self.scroll_focused(-1);
                TerminalEventOutcome::Continue
            }
            KeyCode::Down => {
                self.scroll_focused(1);
                TerminalEventOutcome::Continue
            }
            KeyCode::PageUp => {
                self.scroll_focused(-10);
                TerminalEventOutcome::Continue
            }
            KeyCode::PageDown => {
                self.scroll_focused(10);
                TerminalEventOutcome::Continue
            }
            KeyCode::Backspace
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
            | KeyCode::Modifier(_) => TerminalEventOutcome::Continue,
        }
    }

    fn handle_mouse_event(&mut self, mouse: event::MouseEvent, views: &Views) -> TerminalEventOutcome {
        if self.is_shutdown_mode() || self.prompt.is_some() {
            return TerminalEventOutcome::Continue;
        }

        let point = Rect { x: mouse.column, y: mouse.row, width: 1, height: 1 };

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => self.handle_click(views, point),
            MouseEventKind::ScrollDown => self.handle_scroll(views, point, 3),
            MouseEventKind::ScrollUp => self.handle_scroll(views, point, -3),
            MouseEventKind::Down(_)
            | MouseEventKind::Up(_)
            | MouseEventKind::Drag(_)
            | MouseEventKind::Moved
            | MouseEventKind::ScrollLeft
            | MouseEventKind::ScrollRight => {}
        }

        TerminalEventOutcome::Continue
    }

    fn handle_copy_mode_key(&mut self, key: event::KeyEvent) -> TerminalEventOutcome {
        match key.code {
            KeyCode::Esc => {
                self.exit_copy_mode();
                TerminalEventOutcome::ExitCopyMode
            }
            KeyCode::Char('&') => {
                self.open_prompt(PromptKind::Filter);
                TerminalEventOutcome::Continue
            }
            KeyCode::Char('/') => {
                self.open_prompt(PromptKind::Highlight);
                TerminalEventOutcome::Continue
            }
            KeyCode::Up => {
                self.scroll_focused(-1);
                TerminalEventOutcome::Continue
            }
            KeyCode::Down => {
                self.scroll_focused(1);
                TerminalEventOutcome::Continue
            }
            KeyCode::PageUp => {
                self.scroll_focused(-10);
                TerminalEventOutcome::Continue
            }
            KeyCode::PageDown => {
                self.scroll_focused(10);
                TerminalEventOutcome::Continue
            }
            KeyCode::Backspace
            | KeyCode::Enter
            | KeyCode::Left
            | KeyCode::Right
            | KeyCode::Home
            | KeyCode::End
            | KeyCode::Tab
            | KeyCode::BackTab
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
            | KeyCode::Modifier(_) => TerminalEventOutcome::Continue,
        }
    }

    fn handle_prompt_key(&mut self, key: event::KeyEvent) -> TerminalEventOutcome {
        let Some(prompt) = self.prompt.as_mut() else {
            return TerminalEventOutcome::Continue;
        };

        match prompt.handle_key(key) {
            PromptAction::Continue => TerminalEventOutcome::Continue,
            PromptAction::Cancel => {
                self.prompt = None;
                TerminalEventOutcome::Continue
            }
            PromptAction::Submit => {
                if let Some(prompt) = self.prompt.take() {
                    self.apply_prompt(prompt);
                }
                TerminalEventOutcome::Continue
            }
        }
    }

    fn open_prompt(&mut self, kind: PromptKind) {
        if !self.is_ready(std::time::Instant::now()) {
            return;
        }

        self.scroll_focus = ScrollFocus::Logs;
        let initial = match kind {
            PromptKind::Filter => self.text_filter_pattern.clone(),
            PromptKind::Highlight => self.highlight_pattern.clone(),
        };
        self.prompt = Some(PromptState::new(kind, initial));
    }

    fn apply_prompt(&mut self, prompt: PromptState) {
        let regex = prompt.compiled();
        match prompt.kind {
            PromptKind::Filter => self.set_text_filter(prompt.input, regex),
            PromptKind::Highlight => self.set_highlight(prompt.input, regex),
        }
    }

    fn set_text_filter(&mut self, pattern: String, regex: Option<Regex>) {
        self.text_filter_pattern = if regex.is_some() { pattern } else { String::new() };
        self.text_filter = regex;
        self.sync_logs();
        self.log_scroll = 0;
        self.scroll_focus = ScrollFocus::Logs;
        self.refresh_highlight_cursor();
    }

    fn set_highlight(&mut self, pattern: String, regex: Option<Regex>) {
        self.highlight_pattern = if regex.is_some() { pattern } else { String::new() };
        self.highlight = regex;
        self.scroll_focus = ScrollFocus::Logs;
        self.refresh_highlight_cursor();
    }

    fn refresh_highlight_cursor(&mut self) {
        if self.highlight.is_none() {
            self.log_cursor = None;
            return;
        }

        self.sync_logs();
        self.log_cursor = self.newest_highlight();
        self.scroll_cursor_into_view();
    }

    fn jump_highlight(&mut self, direction: isize) {
        self.sync_logs();
        let Some(regex) = self.highlight.as_ref() else {
            return;
        };

        let matches: Vec<Rc<TelemetryRecord>> = self
            .logs
            .view()
            .iter()
            .filter_map(|item| {
                let record = item.record()?;
                regex.is_match(&record.plain_text()).then(|| Rc::clone(record))
            })
            .collect();
        if matches.is_empty() {
            self.log_cursor = None;
            return;
        }

        let current =
            self.log_cursor.as_ref().and_then(|cursor| matches.iter().position(|record| Rc::ptr_eq(record, cursor)));
        let next = match (current, direction > 0) {
            (Some(index), true) => matches.get(index + 1).or_else(|| matches.first()),
            (Some(index), false) => {
                index.checked_sub(1).and_then(|index| matches.get(index)).or_else(|| matches.last())
            }
            (None, true) => matches.first(),
            (None, false) => matches.last(),
        };

        if let Some(record) = next {
            self.log_cursor = Some(Rc::clone(record));
            self.scroll_cursor_into_view();
        }
    }

    fn newest_highlight(&self) -> Option<Rc<TelemetryRecord>> {
        let regex = self.highlight.as_ref()?;
        self.logs.view().iter().rev().find_map(|item| {
            let record = item.record()?;
            regex.is_match(&record.plain_text()).then(|| Rc::clone(record))
        })
    }

    fn cursor_index(&self) -> Option<usize> {
        let cursor = self.log_cursor.as_ref()?;
        self.logs.view().iter().position(|item| item.record().is_some_and(|record| Rc::ptr_eq(record, cursor)))
    }

    fn scroll_cursor_into_view(&mut self) {
        let Some(index) = self.cursor_index() else {
            return;
        };
        let total = self.logs.view().len();
        let height = self.logs_viewport_rows.max(1);
        let position = index.saturating_add(1).saturating_sub(height);
        self.log_scroll = total.saturating_sub(height).saturating_sub(position);
        self.scroll_focus = ScrollFocus::Logs;
    }

    fn set_scroll_focus(&mut self, focus: ScrollFocus) {
        self.scroll_focus = focus;
    }
}
