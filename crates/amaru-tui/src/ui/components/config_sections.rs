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
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    widgets::{Block, Cell, Row, Table, Widget},
};

use super::super::{
    common::{blit_buffer, panel_borders, panel_padding},
    theme::{border_secondary, emphasis_white_color, striped_row_style, table_header_style},
};
use crate::{model::InteractionMode, startup::ConfigSection};

const BLOCK_HORIZONTAL_CHROME: u16 = 2;
const TABLE_COLUMN_SPACING: u16 = 1;
const TITLE_CHROME: u16 = 4;

pub(in crate::ui) fn render_section_groups(
    frame: &mut Frame<'_>,
    area: Rect,
    groups: &[&[ConfigSection]],
    show_environment: bool,
    scroll: usize,
    mode: InteractionMode,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    if groups.iter().all(|sections| sections.is_empty()) {
        return;
    }

    let sections = groups.iter().flat_map(|group| group.iter()).collect::<Vec<_>>();
    let total_height = sections.iter().map(|section| section_height(section) as usize).sum::<usize>();
    let scroll = scroll.min(total_height.saturating_sub(area.height as usize));
    let offscreen_area = Rect::new(0, 0, area.width, total_height.min(u16::MAX as usize) as u16);
    let mut offscreen = Buffer::empty(offscreen_area);
    let constraints = sections
        .iter()
        .enumerate()
        .map(|(index, section)| {
            if index + 1 == sections.len() {
                Constraint::Min(section.entries.len() as u16 + 3)
            } else {
                Constraint::Length(section.entries.len() as u16 + 3)
            }
        })
        .collect::<Vec<_>>();
    let chunks = Layout::default().direction(Direction::Vertical).constraints(constraints).split(offscreen_area);

    for (chunk, section) in chunks.iter().copied().zip(sections.iter()) {
        render_config_section(&mut offscreen, chunk, section, show_environment, mode);
    }

    blit_buffer(frame, area, &offscreen, scroll);
}

fn render_config_section(
    buffer: &mut Buffer,
    area: Rect,
    section: &ConfigSection,
    show_environment: bool,
    mode: InteractionMode,
) {
    if section.entries.iter().all(|entry| entry.option.is_none() && entry.env_var.is_none()) {
        let rows = section
            .entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                Row::new(vec![
                    Cell::from(entry.label.as_str()).style(Style::default().fg(Color::Rgb(215, 225, 235))),
                    Cell::from(entry.value.clone()).style(Style::default().fg(emphasis_white_color())),
                ])
                .style(striped_row_style(index))
            })
            .collect::<Vec<_>>();

        let table = Table::new(rows, configuration_columns(section, "Parameter", |entry| Some(entry.label.as_str())))
            .column_spacing(TABLE_COLUMN_SPACING)
            .header(Row::new(vec!["Parameter", "Value"]).style(table_header_style(mode)))
            .block(
                Block::default()
                    .title(super::super::theme::block_title(mode, &section.title))
                    .borders(panel_borders(mode))
                    .border_style(border_secondary(mode))
                    .padding(panel_padding(mode)),
            );
        table.render(area, buffer);
        return;
    }

    let table = if show_environment {
        let rows = section
            .entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                Row::new(vec![
                    Cell::from(entry.option.as_deref().unwrap_or("—"))
                        .style(Style::default().fg(Color::Rgb(215, 225, 235))),
                    Cell::from(entry.env_var.as_deref().unwrap_or("—"))
                        .style(Style::default().fg(Color::Rgb(170, 185, 205))),
                    Cell::from(entry.value.clone()).style(Style::default().fg(emphasis_white_color())),
                ])
                .style(striped_row_style(index))
            })
            .collect::<Vec<_>>();

        Table::new(rows, configuration_columns_with_env(section))
            .column_spacing(TABLE_COLUMN_SPACING)
            .header(Row::new(vec!["Option", "Env", "Value"]).style(table_header_style(mode)))
            .block(
                Block::default()
                    .title(super::super::theme::block_title(mode, &section.title))
                    .borders(panel_borders(mode))
                    .border_style(border_secondary(mode))
                    .padding(panel_padding(mode)),
            )
    } else {
        let rows = section
            .entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                Row::new(vec![
                    Cell::from(entry.option.as_deref().unwrap_or("—"))
                        .style(Style::default().fg(Color::Rgb(215, 225, 235))),
                    Cell::from(entry.value.clone()).style(Style::default().fg(emphasis_white_color())),
                ])
                .style(striped_row_style(index))
            })
            .collect::<Vec<_>>();

        Table::new(rows, configuration_columns(section, "Option", |entry| entry.option.as_deref()))
            .column_spacing(TABLE_COLUMN_SPACING)
            .header(Row::new(vec!["Option", "Value"]).style(table_header_style(mode)))
            .block(
                Block::default()
                    .title(super::super::theme::block_title(mode, &section.title))
                    .borders(panel_borders(mode))
                    .border_style(border_secondary(mode))
                    .padding(panel_padding(mode)),
            )
    };

    table.render(area, buffer);
}

/// Return the narrowest panel width that keeps a configuration table's content visible.
pub(in crate::ui) fn sections_content_width(sections: &[ConfigSection], include_environment: bool) -> u16 {
    sections.iter().map(|section| section_content_width(section, include_environment)).max().unwrap_or_default()
}

fn section_content_width(section: &ConfigSection, include_environment: bool) -> u16 {
    let value_width =
        configuration_column_width(std::slice::from_ref(section), "Value", |entry| Some(entry.value.as_str()));
    let table_width = if section.entries.iter().all(|entry| entry.option.is_none() && entry.env_var.is_none()) {
        configuration_column_width(std::slice::from_ref(section), "Parameter", |entry| Some(entry.label.as_str()))
            .saturating_add(TABLE_COLUMN_SPACING)
            .saturating_add(value_width)
    } else {
        let option_width =
            configuration_column_width(std::slice::from_ref(section), "Option", |entry| entry.option.as_deref());
        if include_environment {
            let env_width =
                configuration_column_width(std::slice::from_ref(section), "Env", |entry| entry.env_var.as_deref());
            option_width
                .saturating_add(TABLE_COLUMN_SPACING)
                .saturating_add(env_width)
                .saturating_add(TABLE_COLUMN_SPACING)
                .saturating_add(value_width)
        } else {
            option_width.saturating_add(TABLE_COLUMN_SPACING).saturating_add(value_width)
        }
    };

    table_width.max(section.title.len() as u16 + TITLE_CHROME).saturating_add(BLOCK_HORIZONTAL_CHROME)
}

fn configuration_columns(
    section: &ConfigSection,
    header: &str,
    value: impl Fn(&crate::startup::ConfigEntry) -> Option<&str>,
) -> [Constraint; 2] {
    let identifier_width = configuration_column_width(std::slice::from_ref(section), header, value);
    let value_width =
        configuration_column_width(std::slice::from_ref(section), "Value", |entry| Some(entry.value.as_str()));

    [Constraint::Length(identifier_width), Constraint::Min(value_width)]
}

fn configuration_columns_with_env(section: &ConfigSection) -> [Constraint; 3] {
    let option_width =
        configuration_column_width(std::slice::from_ref(section), "Option", |entry| entry.option.as_deref());
    let env_width = configuration_column_width(std::slice::from_ref(section), "Env", |entry| entry.env_var.as_deref());
    let value_width =
        configuration_column_width(std::slice::from_ref(section), "Value", |entry| Some(entry.value.as_str()));

    [Constraint::Length(option_width), Constraint::Length(env_width), Constraint::Min(value_width)]
}

fn configuration_column_width(
    sections: &[ConfigSection],
    header: &str,
    value: impl Fn(&crate::startup::ConfigEntry) -> Option<&str>,
) -> u16 {
    sections
        .iter()
        .flat_map(|section| section.entries.iter())
        .filter_map(value)
        .map(str::len)
        .chain(std::iter::once(header.len()))
        .max()
        .and_then(|width| u16::try_from(width).ok())
        .unwrap_or(u16::MAX)
}

fn section_height(section: &ConfigSection) -> u16 {
    section.entries.len().saturating_add(3) as u16
}
