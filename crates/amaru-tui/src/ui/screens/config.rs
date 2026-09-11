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
    common::render_scrollbar,
    components::{render_section_groups, sections_content_width},
};
use crate::{model::Model, ui::Views};

const TABLE_GUTTER: u16 = 1;

pub(in crate::ui) fn render_config(frame: &mut Frame<'_>, area: Rect, model: &Model, views: &mut Views) {
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
    views.config_area = content_area;

    let show_environment =
        should_show_environment(content_area, &model.startup.runtime_sections, &model.startup.protocol_sections);
    let columns = config_columns(
        content_area,
        &model.startup.runtime_sections,
        &model.startup.protocol_sections,
        show_environment,
    );

    render_section_groups(
        frame,
        columns[0],
        &[&model.startup.runtime_sections],
        show_environment,
        scroll,
        model.interaction_mode,
    );
    render_section_groups(
        frame,
        columns[1],
        &[&model.startup.protocol_sections],
        false,
        scroll,
        model.interaction_mode,
    );

    if overflowing {
        render_scrollbar(frame, scrollbar_area, total_height, visible_height, scroll, model.interaction_mode, false);
    }
}

fn config_columns(
    area: Rect,
    runtime_sections: &[crate::startup::ConfigSection],
    protocol_sections: &[crate::startup::ConfigSection],
    show_environment: bool,
) -> [Rect; 2] {
    let protocol_width = sections_content_width(protocol_sections, false);
    let runtime_width = sections_content_width(runtime_sections, show_environment);

    if runtime_width.saturating_add(protocol_width).saturating_add(TABLE_GUTTER) <= area.width {
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .spacing(TABLE_GUTTER)
            .constraints([Constraint::Length(runtime_width), Constraint::Min(protocol_width)])
            .split(area);
        [columns[0], columns[1]]
    } else {
        let total_width = runtime_width.saturating_add(protocol_width).max(1);
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Ratio(u32::from(runtime_width), u32::from(total_width)),
                Constraint::Ratio(u32::from(protocol_width), u32::from(total_width)),
            ])
            .split(area);
        [columns[0], columns[1]]
    }
}

fn should_show_environment(
    area: Rect,
    runtime_sections: &[crate::startup::ConfigSection],
    protocol_sections: &[crate::startup::ConfigSection],
) -> bool {
    sections_content_width(runtime_sections, true)
        .saturating_add(sections_content_width(protocol_sections, false))
        .saturating_add(TABLE_GUTTER)
        <= area.width
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::startup::{ConfigEntry, ConfigSection};

    fn configuration_sections() -> (Vec<ConfigSection>, Vec<ConfigSection>) {
        let runtime = vec![ConfigSection::new(
            "Runtime",
            vec![ConfigEntry::new(
                "peer removal cooldown",
                Some("--peer-removal-cooldown-secs"),
                Some("AMARU_PEER_REMOVAL_COOLDOWN_SECS"),
                "/var/lib/amaru/peer-removal-cooldown-seconds",
            )],
        )];
        let protocol = vec![ConfigSection::new(
            "Protocol",
            vec![ConfigEntry::new("maximum transaction size", None::<String>, None::<String>, "16384")],
        )];
        (runtime, protocol)
    }

    #[test]
    fn configuration_tables_give_the_protocol_values_all_remaining_width() {
        let (runtime, protocol) = configuration_sections();
        let area = Rect::new(0, 0, 160, 1);
        assert!(should_show_environment(area, &runtime, &protocol));
        let columns = config_columns(area, &runtime, &protocol, true);

        assert_eq!(columns[0].width, sections_content_width(&runtime, true));
        assert_eq!(columns[1].x, columns[0].x.saturating_add(columns[0].width).saturating_add(TABLE_GUTTER));
        assert_eq!(columns[1].x.saturating_add(columns[1].width), 160);
        assert!(columns[1].width > sections_content_width(&protocol, false));
    }

    #[test]
    fn configuration_tables_hide_environment_before_shrinking_identifiers() {
        let (runtime, protocol) = configuration_sections();
        let runtime_without_environment = sections_content_width(&runtime, false);
        let protocol_width = sections_content_width(&protocol, false);
        let width = runtime_without_environment.saturating_add(protocol_width).saturating_add(TABLE_GUTTER);
        let area = Rect::new(0, 0, width, 1);
        assert!(!should_show_environment(area, &runtime, &protocol));
        let columns = config_columns(area, &runtime, &protocol, false);

        assert_eq!(columns[0].width, runtime_without_environment);
        assert_eq!(columns[1].width, protocol_width);
    }
}
