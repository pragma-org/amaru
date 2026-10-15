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
use crate::{
    model::{InteractionMode, TextPane},
    startup::ConfigSection,
    ui::text_map::{TextMap, column_offsets, push_plain_cell},
};

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
    text: &mut TextMap,
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
    let mut placed = PlacedText { map: text, view: area, scroll };

    for (chunk, section) in chunks.iter().copied().zip(sections.iter()) {
        render_config_section(&mut offscreen, chunk, section, show_environment, mode, &mut placed);
    }

    blit_buffer(frame, area, &offscreen, scroll);
}

struct PlacedText<'a> {
    map: &'a mut TextMap,
    view: Rect,
    scroll: usize,
}

fn render_config_section(
    buffer: &mut Buffer,
    area: Rect,
    section: &ConfigSection,
    show_environment: bool,
    mode: InteractionMode,
    placed: &mut PlacedText<'_>,
) {
    let label = Style::default().fg(Color::Rgb(215, 225, 235));
    let value = Style::default().fg(emphasis_white_color());
    let parameter_only = section.entries.iter().all(|entry| entry.option.is_none() && entry.env_var.is_none());
    let (header, columns, plains, rows) = if parameter_only {
        let plains =
            section.entries.iter().map(|entry| vec![entry.label.clone(), entry.value.clone()]).collect::<Vec<_>>();
        let rows = plains
            .iter()
            .enumerate()
            .map(|(index, cells)| {
                Row::new(vec![Cell::from(cells[0].clone()).style(label), Cell::from(cells[1].clone()).style(value)])
                    .style(striped_row_style(index))
            })
            .collect::<Vec<_>>();
        (
            vec!["Parameter", "Value"],
            configuration_columns(section, "Parameter", |entry| Some(entry.label.as_str())).to_vec(),
            plains,
            rows,
        )
    } else if show_environment {
        let env = Style::default().fg(Color::Rgb(170, 185, 205));
        let plains = section
            .entries
            .iter()
            .map(|entry| {
                vec![
                    entry.option.clone().unwrap_or_else(|| "—".into()),
                    entry.env_var.clone().unwrap_or_else(|| "—".into()),
                    entry.value.clone(),
                ]
            })
            .collect::<Vec<_>>();
        let rows = plains
            .iter()
            .enumerate()
            .map(|(index, cells)| {
                Row::new(vec![
                    Cell::from(cells[0].clone()).style(label),
                    Cell::from(cells[1].clone()).style(env),
                    Cell::from(cells[2].clone()).style(value),
                ])
                .style(striped_row_style(index))
            })
            .collect::<Vec<_>>();
        (vec!["Option", "Env", "Value"], configuration_columns_with_env(section).to_vec(), plains, rows)
    } else {
        let plains = section
            .entries
            .iter()
            .map(|entry| vec![entry.option.clone().unwrap_or_else(|| "—".into()), entry.value.clone()])
            .collect::<Vec<_>>();
        let rows = plains
            .iter()
            .enumerate()
            .map(|(index, cells)| {
                Row::new(vec![Cell::from(cells[0].clone()).style(label), Cell::from(cells[1].clone()).style(value)])
                    .style(striped_row_style(index))
            })
            .collect::<Vec<_>>();
        (
            vec!["Option", "Value"],
            configuration_columns(section, "Option", |entry| entry.option.as_deref()).to_vec(),
            plains,
            rows,
        )
    };

    let block = Block::default()
        .title(super::super::theme::block_title(mode, &section.title))
        .borders(panel_borders(mode))
        .border_style(border_secondary(mode))
        .padding(panel_padding(mode));
    record_config_table(placed, area, &block, &columns, &header, &plains);
    Table::new(rows, columns)
        .column_spacing(TABLE_COLUMN_SPACING)
        .header(Row::new(header).style(table_header_style(mode)))
        .block(block)
        .render(area, buffer);
}

fn record_config_table(
    placed: &mut PlacedText<'_>,
    area: Rect,
    block: &Block<'_>,
    columns: &[Constraint],
    header: &[&str],
    rows: &[Vec<String>],
) {
    let inner = block.inner(area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let offsets = column_offsets(inner.width, columns);
    push_config_line(placed, inner.x, inner.y, &offsets, header);
    for (index, row) in rows.iter().enumerate() {
        let y = inner.y.saturating_add(1).saturating_add(u16::try_from(index).unwrap_or(u16::MAX));
        if y >= inner.y.saturating_add(inner.height) {
            break;
        }
        let cells = row.iter().map(String::as_str).collect::<Vec<_>>();
        push_config_line(placed, inner.x, y, &offsets, &cells);
    }
}

fn push_config_line(placed: &mut PlacedText<'_>, x: u16, y: u16, offsets: &[(u16, u16)], cells: &[&str]) {
    let Some(screen_y) = screen_row(placed.view, placed.scroll, y) else {
        return;
    };
    let record = placed.map.next_record();
    for ((dx, width), plain) in offsets.iter().zip(cells) {
        let screen_x = placed.view.x.saturating_add(x).saturating_add(*dx);
        push_plain_cell(placed.map, screen_x, screen_y, *width, plain, TextPane::Config, record);
    }
}

fn screen_row(view: Rect, scroll: usize, offscreen_y: u16) -> Option<u16> {
    let y = usize::from(offscreen_y);
    if y < scroll {
        return None;
    }
    let local = y - scroll;
    if local >= usize::from(view.height) {
        return None;
    }
    Some(view.y.saturating_add(u16::try_from(local).unwrap_or(u16::MAX)))
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
