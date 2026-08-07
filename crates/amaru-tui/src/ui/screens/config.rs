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

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    widgets::Clear,
};

use super::super::{
    common::{render_scrollbar, show_config_env_column},
    components::render_section_groups,
};
use crate::{model::Model, ui::Views};

pub(in crate::ui) fn render_config(frame: &mut Frame<'_>, area: Rect, model: &Model, views: &mut Views) {
    views.config_area = area;
    frame.render_widget(Clear, area);
    let total_height = page_content_height(model) as usize;
    let visible_height = area.height as usize;
    let scroll = model.config_scroll.min(total_height.saturating_sub(visible_height));
    let overflowing = total_height > visible_height && area.width > 1;
    let (content_area, scrollbar_area) = if overflowing {
        let layout = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(area);
        (layout[0], layout[1])
    } else {
        (area, Rect::default())
    };

    let left_width = if show_config_env_column(content_area) { 60 } else { 64 };
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(left_width), Constraint::Percentage(100 - left_width)])
        .split(content_area);

    render_section_groups(frame, columns[0], &[&model.startup.runtime_sections], scroll, model.interaction_mode);
    render_section_groups(frame, columns[1], &[&model.startup.protocol_sections], scroll, model.interaction_mode);

    if overflowing {
        render_scrollbar(frame, scrollbar_area, total_height, visible_height, scroll, model.interaction_mode);
    }
}

pub(in crate::ui) fn page_content_height(model: &Model) -> u16 {
    config_column_height(&model.startup.runtime_sections).max(config_column_height(&model.startup.protocol_sections))
}

fn config_column_height(sections: &[crate::startup::ConfigSection]) -> u16 {
    sections.iter().map(section_height).sum()
}

fn section_height(section: &crate::startup::ConfigSection) -> u16 {
    section.entries.len().saturating_add(3) as u16
}
