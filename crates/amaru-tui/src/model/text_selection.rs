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

use std::time::{Duration, Instant};

pub(crate) const DOUBLE_CLICK: Duration = Duration::from_millis(400);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TextPane {
    Logs,
    Peers,
    Proposals,
    Config,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TextPress {
    pub x: u16,
    pub y: u16,
    pub pane: TextPane,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TextDrag {
    pub anchor_x: u16,
    pub anchor_y: u16,
    pub x: u16,
    pub y: u16,
    pub pane: TextPane,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TextClick {
    pub at: Instant,
    pub key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HeldCell {
    pub x: u16,
    pub y: u16,
    pub symbol: String,
}

pub(crate) fn is_double_click(previous: Option<&TextClick>, now: Instant, key: &str) -> bool {
    previous.is_some_and(|previous| previous.key == key && now.saturating_duration_since(previous.at) <= DOUBLE_CLICK)
}

impl super::Model {
    pub(crate) fn is_text_dragging(&self) -> bool {
        self.text_drag.is_some()
    }

    pub(crate) fn text_drag_points(&self) -> Option<((u16, u16), (u16, u16))> {
        self.text_drag.map(|drag| ((drag.anchor_x, drag.anchor_y), (drag.x, drag.y)))
    }

    pub(crate) fn copy_status(&self) -> Option<&str> {
        self.copy_status.as_deref()
    }

    pub(crate) fn set_copy_status(&mut self, status: impl Into<String>) {
        self.copy_status = Some(status.into());
    }

    pub(crate) fn held_cells(&self) -> &[HeldCell] {
        &self.held_cells
    }

    pub(crate) fn hold_cells(&mut self, cells: Vec<HeldCell>) {
        self.held_cells = cells;
    }

    pub(crate) fn cancel_text_drag(&mut self) {
        self.text_press = None;
        self.text_drag = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_press_on_the_same_value_within_400ms_is_a_double_click() {
        let at = Instant::now();
        let previous = TextClick { at, key: "token:1:1.2.3.4:3001".into() };
        assert!(is_double_click(Some(&previous), at + Duration::from_millis(400), &previous.key));
        assert!(!is_double_click(Some(&previous), at + Duration::from_millis(401), &previous.key));
        assert!(!is_double_click(Some(&previous), at + Duration::from_millis(10), "token:1:other"));
        assert!(!is_double_click(None, at, &previous.key));
    }
}
