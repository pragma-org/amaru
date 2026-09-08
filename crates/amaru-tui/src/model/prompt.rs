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

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use regex::Regex;

use super::log_time::TimeJump;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    Filter,
    Highlight,
    JumpTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptAction {
    Continue,
    Submit,
    Cancel,
}

#[derive(Debug, Clone)]
pub struct PromptState {
    pub kind: PromptKind,
    pub input: String,
    pub cursor: usize,
    pub error: Option<String>,
}

impl PromptState {
    pub fn new(kind: PromptKind, initial: impl Into<String>) -> Self {
        let input = initial.into();
        let cursor = input.chars().count();
        let mut prompt = Self { kind, input, cursor, error: None };
        prompt.recompile();
        prompt
    }

    pub fn prefix(&self) -> &'static str {
        match self.kind {
            PromptKind::Filter => "& ",
            PromptKind::Highlight => "/ ",
            PromptKind::JumpTime => "@ ",
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> PromptAction {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('u') | KeyCode::Char('U') => {
                    self.input.clear();
                    self.cursor = 0;
                    self.recompile();
                    return PromptAction::Continue;
                }
                KeyCode::Char('w') | KeyCode::Char('W') => {
                    self.delete_word();
                    self.recompile();
                    return PromptAction::Continue;
                }
                KeyCode::Backspace
                | KeyCode::Enter
                | KeyCode::Left
                | KeyCode::Right
                | KeyCode::Up
                | KeyCode::Down
                | KeyCode::Home
                | KeyCode::End
                | KeyCode::PageUp
                | KeyCode::PageDown
                | KeyCode::Tab
                | KeyCode::BackTab
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
                | KeyCode::Modifier(_) => return PromptAction::Continue,
            }
        }

        match key.code {
            KeyCode::Esc => PromptAction::Cancel,
            KeyCode::Enter => {
                self.recompile();
                if self.input.is_empty() || self.error.is_none() {
                    PromptAction::Submit
                } else {
                    PromptAction::Continue
                }
            }
            KeyCode::Backspace => {
                self.delete_left();
                self.recompile();
                PromptAction::Continue
            }
            KeyCode::Delete => {
                self.delete_right();
                self.recompile();
                PromptAction::Continue
            }
            KeyCode::Left => {
                self.cursor = self.cursor.saturating_sub(1);
                PromptAction::Continue
            }
            KeyCode::Right => {
                self.cursor = (self.cursor + 1).min(self.input.chars().count());
                PromptAction::Continue
            }
            KeyCode::Home => {
                self.cursor = 0;
                PromptAction::Continue
            }
            KeyCode::End => {
                self.cursor = self.input.chars().count();
                PromptAction::Continue
            }
            KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::ALT) => {
                self.insert(ch);
                self.recompile();
                PromptAction::Continue
            }
            KeyCode::Up
            | KeyCode::Down
            | KeyCode::PageUp
            | KeyCode::PageDown
            | KeyCode::Tab
            | KeyCode::BackTab
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
            | KeyCode::Modifier(_) => PromptAction::Continue,
        }
    }

    pub fn compiled(&self) -> Option<Regex> {
        if matches!(self.kind, PromptKind::JumpTime) || self.input.is_empty() || self.error.is_some() {
            None
        } else {
            Regex::new(&self.input).ok()
        }
    }

    pub fn parsed_time(&self) -> Option<TimeJump> {
        if !matches!(self.kind, PromptKind::JumpTime) || self.input.is_empty() || self.error.is_some() {
            None
        } else {
            TimeJump::parse(&self.input).ok()
        }
    }

    fn recompile(&mut self) {
        if self.input.is_empty() {
            self.error = None;
            return;
        }

        self.error = match self.kind {
            PromptKind::Filter | PromptKind::Highlight => {
                Regex::new(&self.input).err().map(|error| truncate_error(&error.to_string()))
            }
            PromptKind::JumpTime => TimeJump::parse(&self.input).err().map(|error| truncate_error(&error)),
        };
    }

    fn insert(&mut self, ch: char) {
        let index = char_byte_index(&self.input, self.cursor);
        self.input.insert(index, ch);
        self.cursor += 1;
    }

    fn delete_left(&mut self) {
        if self.cursor == 0 {
            return;
        }

        let end = char_byte_index(&self.input, self.cursor);
        self.cursor -= 1;
        let start = char_byte_index(&self.input, self.cursor);
        self.input.replace_range(start..end, "");
    }

    fn delete_right(&mut self) {
        let chars = self.input.chars().count();
        if self.cursor >= chars {
            return;
        }

        let start = char_byte_index(&self.input, self.cursor);
        let end = char_byte_index(&self.input, self.cursor + 1);
        self.input.replace_range(start..end, "");
    }

    fn delete_word(&mut self) {
        if self.cursor == 0 {
            return;
        }

        let chars: Vec<char> = self.input.chars().collect();
        let mut index = self.cursor;
        while index > 0 && chars[index - 1].is_whitespace() {
            index -= 1;
        }
        while index > 0 && !chars[index - 1].is_whitespace() {
            index -= 1;
        }
        let start = char_byte_index(&self.input, index);
        let end = char_byte_index(&self.input, self.cursor);
        self.input.replace_range(start..end, "");
        self.cursor = index;
    }
}

fn char_byte_index(input: &str, cursor: usize) -> usize {
    input.char_indices().nth(cursor).map(|(index, _)| index).unwrap_or(input.len())
}

fn truncate_error(error: &str) -> String {
    const MAX: usize = 48;
    if error.chars().count() <= MAX {
        error.to_string()
    } else {
        let truncated: String = error.chars().take(MAX.saturating_sub(1)).collect();
        format!("{truncated}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn enter_submits_valid_or_empty_regex() {
        let mut prompt = PromptState::new(PromptKind::Filter, "peer");
        assert_eq!(prompt.handle_key(key(KeyCode::Enter)), PromptAction::Submit);
        assert!(prompt.compiled().is_some());

        prompt = PromptState::new(PromptKind::Filter, "");
        assert_eq!(prompt.handle_key(key(KeyCode::Enter)), PromptAction::Submit);
        assert!(prompt.compiled().is_none());
    }

    #[test]
    fn enter_keeps_invalid_regex_open() {
        let mut prompt = PromptState::new(PromptKind::Highlight, "(");
        assert!(prompt.error.is_some());
        assert_eq!(prompt.handle_key(key(KeyCode::Enter)), PromptAction::Continue);
    }

    #[test]
    fn jump_time_accepts_clock_and_rejects_garbage() {
        let mut prompt = PromptState::new(PromptKind::JumpTime, "13:24");
        assert!(prompt.parsed_time().is_some());
        assert_eq!(prompt.handle_key(key(KeyCode::Enter)), PromptAction::Submit);

        prompt = PromptState::new(PromptKind::JumpTime, "nope");
        assert!(prompt.error.is_some());
        assert_eq!(prompt.handle_key(key(KeyCode::Enter)), PromptAction::Continue);
    }
}
