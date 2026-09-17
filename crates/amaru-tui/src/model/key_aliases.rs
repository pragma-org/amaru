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

use std::env;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub(crate) const ENVIRONMENT_VARIABLE: &str = "AMARU_TUI_ALIASES";

/// User-provided replacements for the TUI's built-in keyboard controls.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct KeyAliases {
    aliases: Vec<(KeyBinding, KeyBinding)>,
}

impl KeyAliases {
    pub(crate) fn from_environment() -> Result<Self, String> {
        match env::var(ENVIRONMENT_VARIABLE) {
            Ok(value) => Self::parse(&value),
            Err(env::VarError::NotPresent) => Ok(Self::default()),
            Err(env::VarError::NotUnicode(_)) => Err("must contain valid Unicode".to_owned()),
        }
    }

    pub(crate) fn parse(input: &str) -> Result<Self, String> {
        let mut aliases = Vec::new();

        for entry in input.split(',').map(str::trim).filter(|entry| !entry.is_empty()) {
            let Some((source, alias)) = entry.split_once('=') else {
                return Err(format!("expected `key=alias`, got `{entry}`"));
            };
            let source = KeyBinding::parse(source.trim())?;
            let alias = KeyBinding::parse(alias.trim())?;

            if !known_bindings().contains(&source) {
                return Err(format!("`{source}` is not an aliasable TUI control"));
            }
            if aliases.iter().any(|(existing, _)| existing == &source) {
                return Err(format!("`{source}` has more than one alias"));
            }

            aliases.push((source, alias));
        }

        let aliases = Self { aliases };
        aliases.validate_conflicts()?;
        Ok(aliases)
    }

    /// Translate a physical key event into the built-in control it replaces.
    ///
    /// A configured alias replaces the original binding rather than adding a second one. This
    /// keeps the configured controls unambiguous, including in prompt editing mode.
    pub(crate) fn translate(&self, mut key: KeyEvent) -> Option<KeyEvent> {
        let actual = KeyBinding::from_event(&key);

        if let Some((source, _)) = self.aliases.iter().find(|(_, alias)| alias == &actual) {
            source.apply_to(&mut key);
            return Some(key);
        }

        self.aliases.iter().all(|(source, _)| source != &actual).then_some(key)
    }

    pub(crate) fn label(&self, source: &str) -> Option<String> {
        let source = KeyBinding::parse(source).ok()?;
        Some(self.resolve(&source).to_string())
    }

    pub(crate) fn page_navigation_label(&self) -> &'static str {
        if self.any_aliased([KeyBinding::tab(), KeyBinding::back_tab()]) { "aliased" } else { "[shift+]tab" }
    }

    pub(crate) fn scroll_navigation_label(&self) -> &'static str {
        if self.any_aliased([
            KeyBinding::up(),
            KeyBinding::down(),
            KeyBinding::left(),
            KeyBinding::right(),
            KeyBinding::control_up(),
            KeyBinding::control_down(),
            KeyBinding::control_left(),
            KeyBinding::control_right(),
            KeyBinding::page_up(),
            KeyBinding::page_down(),
            KeyBinding::home(),
            KeyBinding::end(),
        ]) {
            "aliased"
        } else {
            "[ctrl+]←→↑↓"
        }
    }

    fn resolve(&self, source: &KeyBinding) -> KeyBinding {
        self.aliases
            .iter()
            .find_map(|(candidate, alias)| (candidate == source).then(|| alias.clone()))
            .unwrap_or_else(|| source.clone())
    }

    fn any_aliased<const N: usize>(&self, bindings: [KeyBinding; N]) -> bool {
        bindings.into_iter().any(|binding| self.resolve(&binding) != binding)
    }

    fn validate_conflicts(&self) -> Result<(), String> {
        let bindings = known_bindings();

        for (index, source) in bindings.iter().enumerate() {
            let resolved = self.resolve(source);
            if let Some(other) = bindings[..index].iter().find(|other| self.resolve(other) == resolved) {
                return Err(format!("`{source}` and `{other}` both resolve to `{resolved}`"));
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct KeyBinding {
    code: KeyCode,
    modifiers: KeyModifiers,
}

impl KeyBinding {
    fn parse(value: &str) -> Result<Self, String> {
        let (modifiers, name) = parse_modifiers(value)?;
        let code = match name.to_ascii_lowercase().as_str() {
            "esc" | "escape" => KeyCode::Esc,
            "enter" | "return" => KeyCode::Enter,
            "tab" => KeyCode::Tab,
            "backspace" => KeyCode::Backspace,
            "delete" | "del" => KeyCode::Delete,
            "insert" | "ins" => KeyCode::Insert,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "pageup" | "pgup" => KeyCode::PageUp,
            "pagedown" | "pgdown" => KeyCode::PageDown,
            "space" => KeyCode::Char(' '),
            name if name.starts_with('f') => {
                let number = name[1..]
                    .parse::<u8>()
                    .map_err(|_| format!("unknown key `{value}`; expected a key name or one character"))?;
                KeyCode::F(number)
            }
            name => {
                let mut characters = name.chars();
                let Some(character) = characters.next() else {
                    return Err("key cannot be empty".to_owned());
                };
                if characters.next().is_some() {
                    return Err(format!("unknown key `{value}`; expected a key name or one character"));
                }
                KeyCode::Char(character)
            }
        };

        Ok(Self::new(code, modifiers))
    }

    fn from_event(key: &KeyEvent) -> Self {
        Self::new(key.code, key.modifiers)
    }

    fn new(mut code: KeyCode, mut modifiers: KeyModifiers) -> Self {
        if code == KeyCode::Tab && modifiers.contains(KeyModifiers::SHIFT) {
            code = KeyCode::BackTab;
            modifiers.remove(KeyModifiers::SHIFT);
        }

        if let KeyCode::Char(character) = &code {
            if character.is_ascii_alphabetic() {
                if character.is_ascii_uppercase() {
                    code = KeyCode::Char(character.to_ascii_lowercase());
                    modifiers.insert(KeyModifiers::SHIFT);
                }
            } else {
                // Crossterm reports Shift for punctuation such as `~`, although the character
                // already fully identifies the key the user configured.
                modifiers.remove(KeyModifiers::SHIFT);
            }
        }

        Self { code, modifiers }
    }

    fn apply_to(&self, key: &mut KeyEvent) {
        key.code = self.code;
        key.modifiers = self.modifiers;
    }

    fn esc() -> Self {
        Self::new(KeyCode::Esc, KeyModifiers::NONE)
    }

    fn enter() -> Self {
        Self::new(KeyCode::Enter, KeyModifiers::NONE)
    }

    fn tab() -> Self {
        Self::new(KeyCode::Tab, KeyModifiers::NONE)
    }

    fn back_tab() -> Self {
        Self::new(KeyCode::BackTab, KeyModifiers::NONE)
    }

    fn up() -> Self {
        Self::new(KeyCode::Up, KeyModifiers::NONE)
    }

    fn down() -> Self {
        Self::new(KeyCode::Down, KeyModifiers::NONE)
    }

    fn left() -> Self {
        Self::new(KeyCode::Left, KeyModifiers::NONE)
    }

    fn right() -> Self {
        Self::new(KeyCode::Right, KeyModifiers::NONE)
    }

    fn control_up() -> Self {
        Self::new(KeyCode::Up, KeyModifiers::CONTROL)
    }

    fn control_down() -> Self {
        Self::new(KeyCode::Down, KeyModifiers::CONTROL)
    }

    fn control_left() -> Self {
        Self::new(KeyCode::Left, KeyModifiers::CONTROL)
    }

    fn control_right() -> Self {
        Self::new(KeyCode::Right, KeyModifiers::CONTROL)
    }

    fn page_up() -> Self {
        Self::new(KeyCode::PageUp, KeyModifiers::NONE)
    }

    fn page_down() -> Self {
        Self::new(KeyCode::PageDown, KeyModifiers::NONE)
    }

    fn home() -> Self {
        Self::new(KeyCode::Home, KeyModifiers::NONE)
    }

    fn end() -> Self {
        Self::new(KeyCode::End, KeyModifiers::NONE)
    }
}

impl std::fmt::Display for KeyBinding {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.modifiers.contains(KeyModifiers::CONTROL) {
            write!(formatter, "ctrl+")?;
        }

        if self.code == KeyCode::BackTab {
            return write!(formatter, "shift+tab");
        }

        if self.modifiers.contains(KeyModifiers::SHIFT) {
            write!(formatter, "shift+")?;
        }

        if let KeyCode::Char(character) = self.code {
            return write!(formatter, "{character}");
        }

        if let KeyCode::F(number) = self.code {
            return write!(formatter, "f{number}");
        }

        formatter.write_str(key_name(&self.code))
    }
}

fn parse_modifiers(value: &str) -> Result<(KeyModifiers, &str), String> {
    let mut modifiers = KeyModifiers::NONE;
    let mut remaining = value;

    while let Some((prefix, suffix)) = remaining.split_once('+') {
        match prefix.to_ascii_lowercase().as_str() {
            "ctrl" if !modifiers.contains(KeyModifiers::CONTROL) => modifiers.insert(KeyModifiers::CONTROL),
            "shift" if !modifiers.contains(KeyModifiers::SHIFT) => modifiers.insert(KeyModifiers::SHIFT),
            "ctrl" | "shift" => return Err(format!("repeated modifier in `{value}`")),
            _ => break,
        }
        remaining = suffix;
    }

    (!remaining.is_empty())
        .then_some((modifiers, remaining))
        .ok_or_else(|| format!("key `{value}` is missing a name after its modifier"))
}

fn key_name(code: &KeyCode) -> &str {
    match code {
        KeyCode::BackTab => "backtab",
        KeyCode::Esc => "esc",
        KeyCode::Enter => "enter",
        KeyCode::Tab => "tab",
        KeyCode::Backspace => "backspace",
        KeyCode::Delete => "delete",
        KeyCode::Insert => "insert",
        KeyCode::Home => "home",
        KeyCode::End => "end",
        KeyCode::Up => "up",
        KeyCode::Down => "down",
        KeyCode::Left => "left",
        KeyCode::Right => "right",
        KeyCode::PageUp => "pageup",
        KeyCode::PageDown => "pagedown",
        KeyCode::F(_) => "function",
        KeyCode::Char(_) => "character",
        KeyCode::Null => "null",
        KeyCode::CapsLock => "capslock",
        KeyCode::ScrollLock => "scrolllock",
        KeyCode::NumLock => "numlock",
        KeyCode::PrintScreen => "printscreen",
        KeyCode::Pause => "pause",
        KeyCode::Menu => "menu",
        KeyCode::KeypadBegin => "keypadbegin",
        KeyCode::Media(_) => "media",
        KeyCode::Modifier(_) => "modifier",
    }
}

fn known_bindings() -> [KeyBinding; 24] {
    [
        KeyBinding::esc(),
        KeyBinding::enter(),
        KeyBinding::tab(),
        KeyBinding::back_tab(),
        KeyBinding::up(),
        KeyBinding::down(),
        KeyBinding::left(),
        KeyBinding::right(),
        KeyBinding::control_up(),
        KeyBinding::control_down(),
        KeyBinding::control_left(),
        KeyBinding::control_right(),
        KeyBinding::page_up(),
        KeyBinding::page_down(),
        KeyBinding::home(),
        KeyBinding::end(),
        KeyBinding::new(KeyCode::Char(';'), KeyModifiers::NONE),
        KeyBinding::new(KeyCode::Char('f'), KeyModifiers::NONE),
        KeyBinding::new(KeyCode::Char('q'), KeyModifiers::NONE),
        KeyBinding::new(KeyCode::Char('h'), KeyModifiers::NONE),
        KeyBinding::new(KeyCode::Char('t'), KeyModifiers::NONE),
        KeyBinding::new(KeyCode::Char('w'), KeyModifiers::NONE),
        KeyBinding::new(KeyCode::Char('y'), KeyModifiers::NONE),
        KeyBinding::new(KeyCode::Char('n'), KeyModifiers::NONE),
    ]
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::KeyAliases;

    #[test]
    fn aliases_replace_controls_and_render_their_binding() {
        let aliases = KeyAliases::parse("esc=~,ctrl+up=j,ctrl+down=k").unwrap();

        let escape = aliases.translate(KeyEvent::new(KeyCode::Char('~'), KeyModifiers::SHIFT)).unwrap();
        assert_eq!(escape.code, KeyCode::Esc);
        assert_eq!(escape.modifiers, KeyModifiers::NONE);
        assert!(aliases.translate(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)).is_none());

        assert_eq!(aliases.label("esc").as_deref(), Some("~"));
        assert_eq!(aliases.scroll_navigation_label(), "aliased");
        assert_eq!(aliases.page_navigation_label(), "[shift+]tab");
    }

    #[test]
    fn shift_aliases_normalize_uppercase_events() {
        let aliases = KeyAliases::parse("esc=shift+x").unwrap();

        let escape = aliases.translate(KeyEvent::new(KeyCode::Char('X'), KeyModifiers::SHIFT)).unwrap();
        assert_eq!(escape.code, KeyCode::Esc);
        assert_eq!(escape.modifiers, KeyModifiers::NONE);
        assert_eq!(aliases.label("esc").as_deref(), Some("shift+x"));
    }

    #[test]
    fn aliases_reject_conflicting_controls() {
        let error = KeyAliases::parse("ctrl+up=j,ctrl+down=j").unwrap_err();

        assert_eq!(error, "`ctrl+down` and `ctrl+up` both resolve to `j`");
    }

    #[test]
    fn aliases_reject_unknown_sources() {
        let error = KeyAliases::parse("a=j").unwrap_err();

        assert_eq!(error, "`a` is not an aliasable TUI control");
    }
}
