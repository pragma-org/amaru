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
    widgets::{Block, Borders, Cell, Row, Table, Widget},
};

use super::super::{
    common::{blit_buffer, show_config_env_column},
    theme::{border_secondary, emphasis_white_color, striped_row_style, table_header_style},
};
use crate::{model::InteractionMode, startup::ConfigSection};

pub(in crate::ui) fn render_section_groups(
    frame: &mut Frame<'_>,
    area: Rect,
    groups: &[&[ConfigSection]],
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
        render_config_section(&mut offscreen, chunk, section, mode);
    }

    blit_buffer(frame, area, &offscreen, scroll);
}

fn render_config_section(buffer: &mut Buffer, area: Rect, section: &ConfigSection, mode: InteractionMode) {
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

        let table = Table::new(rows, [Constraint::Length(28), Constraint::Min(10)])
            .header(Row::new(vec!["Parameter", "Value"]).style(table_header_style(mode)))
            .block(
                Block::default()
                    .title(super::super::theme::block_title(mode, &section.title))
                    .borders(Borders::ALL)
                    .border_style(border_secondary(mode)),
            );
        table.render(area, buffer);
        return;
    }

    let table = if show_config_env_column(area) {
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

        Table::new(rows, [Constraint::Length(28), Constraint::Length(34), Constraint::Min(10)])
            .header(Row::new(vec!["Option", "Env", "Value"]).style(table_header_style(mode)))
            .block(
                Block::default()
                    .title(super::super::theme::block_title(mode, &section.title))
                    .borders(Borders::ALL)
                    .border_style(border_secondary(mode)),
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

        Table::new(rows, [Constraint::Length(28), Constraint::Min(10)])
            .header(Row::new(vec!["Option", "Value"]).style(table_header_style(mode)))
            .block(
                Block::default()
                    .title(super::super::theme::block_title(mode, &section.title))
                    .borders(Borders::ALL)
                    .border_style(border_secondary(mode)),
            )
    };

    table.render(area, buffer);
}

fn section_height(section: &ConfigSection) -> u16 {
    section.entries.len().saturating_add(3) as u16
}
