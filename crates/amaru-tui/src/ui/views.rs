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

use ratatui::layout::Rect;

use crate::model::{LevelFilter, Page, ScrollFocus, TargetFilter};

#[derive(Debug, Default, Clone)]
pub struct Views {
    pub page_tabs: Vec<(Page, Rect)>,
    pub log_toggle: Rect,
    pub peer_toggle: Rect,
    pub proposal_toggle: Rect,
    pub window_tabs: Vec<Rect>,
    pub level_tabs: Vec<(LevelFilter, Rect)>,
    pub target_tabs: Vec<(TargetFilter, Rect)>,
    pub logs_area: Rect,
    pub peers_area: Rect,
    pub proposals_area: Rect,
}

impl Views {
    pub fn reset(&mut self) {
        self.page_tabs.clear();
        self.window_tabs.clear();
        self.level_tabs.clear();
        self.target_tabs.clear();
        self.log_toggle = Rect::default();
        self.peer_toggle = Rect::default();
        self.proposal_toggle = Rect::default();
        self.logs_area = Rect::default();
        self.peers_area = Rect::default();
        self.proposals_area = Rect::default();
    }

    pub fn page_at(&self, point: Rect) -> Option<Page> {
        self.page_tabs.iter().find_map(|(page, area)| contains(*area, point).then_some(*page))
    }

    pub fn toggles_logs(&self, point: Rect) -> bool {
        contains(self.log_toggle, point)
    }

    pub fn toggles_peers(&self, point: Rect) -> bool {
        contains(self.peer_toggle, point)
    }

    pub fn toggles_proposals(&self, point: Rect) -> bool {
        contains(self.proposal_toggle, point)
    }

    pub fn window_at(&self, point: Rect) -> Option<usize> {
        self.window_tabs.iter().position(|area| contains(*area, point))
    }

    pub fn level_filter_at(&self, point: Rect) -> Option<LevelFilter> {
        self.level_tabs.iter().find_map(|(filter, area)| contains(*area, point).then_some(*filter))
    }

    pub fn target_filter_at(&self, point: Rect) -> Option<TargetFilter> {
        self.target_tabs.iter().find_map(|(filter, area)| contains(*area, point).then_some(*filter))
    }

    pub fn focus_at(&self, point: Rect) -> Option<ScrollFocus> {
        if contains(self.logs_area, point) {
            Some(ScrollFocus::Logs)
        } else if contains(self.peers_area, point) {
            Some(ScrollFocus::Peers)
        } else if contains(self.proposals_area, point) {
            Some(ScrollFocus::Proposals)
        } else {
            None
        }
    }

    pub fn scroll_focus_at(&self, point: Rect) -> ScrollFocus {
        self.focus_at(point).unwrap_or(ScrollFocus::Logs)
    }
}

fn contains(area: Rect, point: Rect) -> bool {
    point.x >= area.x && point.x < area.x + area.width && point.y >= area.y && point.y < area.y + area.height
}
