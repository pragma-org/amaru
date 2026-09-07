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

use std::time::Instant;

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders},
};

use self::{
    common::{accent_primary, border_title_line, border_title_prefix_width, button_label},
    components::{render_epoch_progress, render_logs, render_peers_table, render_proposals_table},
    screens::{render_amaru, render_cardano, render_config, render_splash},
    theme::{border_primary, emphasis_primary, emphasis_white, emphasis_white_color},
};
use crate::model::{Model, Page};

mod common;
mod components;
mod format;
mod screens;
mod theme;
mod views;

pub use self::views::Views;

pub fn render(frame: &mut Frame<'_>, model: &Model, views: &mut Views, now: Instant) {
    views.reset();

    let is_ready = model.is_ready(now);
    let progress_height = u16::from(model.tip.is_some()) * 3;
    let shell = shell_block(model, is_ready);
    let shell_area = frame.area();
    let inner = shell.inner(shell_area);

    frame.render_widget(shell, shell_area);
    if model.prompt_is_open() {
        set_prompt_cursor(frame, shell_area, model);
    }
    if !is_ready {
        render_splash(frame, inner, model, views);
        if model.is_shutdown_mode() {
            apply_shutdown_overlay(frame, inner);
        }
        return;
    }

    populate_shell_hotspots(views, shell_area, model);

    if model.page == Page::Amaru && model.peer_pane_mode.is_maximized() {
        render_peers_table(frame, inner, model, views, now);
        if model.is_shutdown_mode() {
            apply_shutdown_overlay(frame, inner);
        }
        return;
    }

    if model.page == Page::Cardano && model.proposal_pane_mode.is_maximized() {
        render_proposals_table(frame, inner, model, views);
        if model.is_shutdown_mode() {
            apply_shutdown_overlay(frame, inner);
        }
        return;
    }

    if model.log_pane_mode.is_maximized() && model.page != Page::Config {
        render_logs(frame, inner, model, views);
        if model.is_shutdown_mode() {
            apply_shutdown_overlay(frame, inner);
        }
        return;
    }

    let show_logs = model.page != Page::Config;

    if show_logs {
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(progress_height),
                Constraint::Length(page_content_height(model)),
                Constraint::Fill(1),
            ])
            .split(inner);

        if progress_height > 0 {
            render_epoch_progress(frame, layout[0], model);
        }

        if is_ready {
            match model.page {
                Page::Amaru => render_amaru(frame, layout[1], model, views, now),
                Page::Cardano => render_cardano(frame, layout[1], model, views, now),
                Page::Config => render_config(frame, layout[1], model, views),
            }
        } else {
            render_splash(frame, layout[1], model, views);
        }

        render_logs(frame, layout[2], model, views);
    } else {
        let available_height = inner.height;
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(available_height)])
            .split(inner);

        if is_ready {
            render_config(frame, layout[0], model, views);
        } else {
            render_splash(frame, layout[0], model, views);
        }
    }

    if model.is_shutdown_mode() {
        apply_shutdown_overlay(frame, inner);
    }
}

fn shell_block(model: &Model, is_ready: bool) -> Block<'static> {
    let block = Block::default().borders(Borders::ALL).border_style(border_primary(model.interaction_mode));

    if is_ready {
        let block = block.title_top(page_tabs_line(model).left_aligned()).title_top(shell_title(model).centered());
        if model.prompt_is_open() {
            block.title_bottom(prompt_line(model).left_aligned())
        } else {
            block.title_bottom(shell_hint(model).right_aligned())
        }
    } else {
        block.title_top(shell_title(model).centered())
    }
}

fn populate_shell_hotspots(views: &mut Views, area: Rect, _model: &Model) {
    let mut x = area.x + 2 + border_title_prefix_width();
    let y = area.y;
    for (index, page) in Page::ALL.into_iter().enumerate() {
        let label = button_label(page.label());
        views.page_tabs.push((page, Rect { x, y, width: label.len() as u16, height: 1 }));
        x += label.len() as u16;
        if index + 1 != Page::ALL.len() {
            x += 1;
        }
    }
}

fn page_tabs_line(model: &Model) -> Line<'static> {
    let mut spans = Vec::new();

    for (index, page) in Page::ALL.into_iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw(" "));
        }
        let style = if page == model.page {
            emphasis_primary(model.interaction_mode)
        } else {
            Style::default().fg(Color::Rgb(185, 198, 214)).add_modifier(Modifier::BOLD)
        };
        spans.push(Span::styled(button_label(page.label()), style));
    }

    border_title_line(spans, model.interaction_mode, false)
}

fn shell_title(model: &Model) -> Line<'static> {
    if model.is_shutdown_mode() {
        return border_title_line(
            vec![Span::styled(
                " SHUTTING DOWN ",
                Style::default()
                    .fg(emphasis_white_color())
                    .bg(accent_primary(model.interaction_mode))
                    .add_modifier(Modifier::BOLD),
            )],
            model.interaction_mode,
            false,
        );
    }

    if model.is_copy_mode() {
        return border_title_line(
            vec![Span::styled(
                " COPY MODE ",
                Style::default()
                    .fg(emphasis_white_color())
                    .bg(accent_primary(model.interaction_mode))
                    .add_modifier(Modifier::BOLD),
            )],
            model.interaction_mode,
            false,
        );
    }

    border_title_line(
        vec![
            Span::styled("AMARU", emphasis_primary(model.interaction_mode)),
            Span::raw("  "),
            Span::styled(model.startup.process.software_version.clone(), emphasis_white()),
        ],
        model.interaction_mode,
        false,
    )
}

fn shell_hint(model: &Model) -> Line<'static> {
    if model.is_shutdown_mode() {
        return border_title_line(
            vec![Span::styled("please wait", theme::muted().add_modifier(Modifier::BOLD))],
            model.interaction_mode,
            false,
        );
    }

    if model.log_scrollbar_focused {
        return border_title_line(
            vec![
                Span::styled("<|>", emphasis_primary(model.interaction_mode)),
                Span::styled(" LOGS  ", theme::muted()),
                Span::styled("<↑↓>", emphasis_primary(model.interaction_mode)),
                Span::styled(" SCRUB  ", theme::muted()),
                Span::styled("<pgup/pgdn>", emphasis_primary(model.interaction_mode)),
                Span::styled(" PAGE  ", theme::muted()),
                Span::styled("<home/end>", emphasis_primary(model.interaction_mode)),
                Span::styled(" ENDS  ", theme::muted()),
                Span::styled("<@>", emphasis_primary(model.interaction_mode)),
                Span::styled(" TIME", theme::muted()),
            ],
            model.interaction_mode,
            false,
        );
    }

    if model.is_copy_mode() {
        let arrow_hint = if model.highlight_pattern.is_empty() { " SCROLL  " } else { " MATCHES  " };
        return border_title_line(
            vec![
                Span::styled("<esc>", emphasis_primary(model.interaction_mode)),
                Span::styled(" NORMAL  ", theme::muted()),
                Span::styled("<↑↓>", emphasis_primary(model.interaction_mode)),
                Span::styled(arrow_hint, theme::muted()),
                Span::styled("<home/end>", emphasis_primary(model.interaction_mode)),
                Span::styled(" ENDS  ", theme::muted()),
                Span::styled("<|>", emphasis_primary(model.interaction_mode)),
                Span::styled(" SCRUB  ", theme::muted()),
                Span::styled("<&>", emphasis_primary(model.interaction_mode)),
                Span::styled(" FILTER  ", theme::muted()),
                Span::styled("</>", emphasis_primary(model.interaction_mode)),
                Span::styled(" HIGHLIGHT  ", theme::muted()),
                Span::styled("<@>", emphasis_primary(model.interaction_mode)),
                Span::styled(" TIME", theme::muted()),
            ],
            model.interaction_mode,
            false,
        );
    }

    let arrow_hint = if model.highlight_pattern.is_empty() { " SCROLL  " } else { " MATCHES  " };
    border_title_line(
        vec![
            Span::styled("<mouse>", emphasis_primary(model.interaction_mode)),
            Span::styled(" NAV  ", theme::muted()),
            Span::styled("<esc>", emphasis_primary(model.interaction_mode)),
            Span::styled(" COPY  ", theme::muted()),
            Span::styled("<tab>", emphasis_primary(model.interaction_mode)),
            Span::styled(" PAGE  ", theme::muted()),
            Span::styled("<←→>", emphasis_primary(model.interaction_mode)),
            Span::styled(" FOCUS  ", theme::muted()),
            Span::styled("<↑↓>", emphasis_primary(model.interaction_mode)),
            Span::styled(arrow_hint, theme::muted()),
            Span::styled("<enter>", emphasis_primary(model.interaction_mode)),
            Span::styled(" MAX  ", theme::muted()),
            Span::styled("<|>", emphasis_primary(model.interaction_mode)),
            Span::styled(" SCRUB  ", theme::muted()),
            Span::styled("<&>", emphasis_primary(model.interaction_mode)),
            Span::styled(" FILTER  ", theme::muted()),
            Span::styled("</>", emphasis_primary(model.interaction_mode)),
            Span::styled(" HIGHLIGHT  ", theme::muted()),
            Span::styled("<@>", emphasis_primary(model.interaction_mode)),
            Span::styled(" TIME  ", theme::muted()),
            Span::styled("<q>", emphasis_primary(model.interaction_mode)),
            Span::styled(" QUIT", theme::muted()),
        ],
        model.interaction_mode,
        false,
    )
}

fn prompt_line(model: &Model) -> Line<'static> {
    let Some(prompt) = model.prompt.as_ref() else {
        return Line::default();
    };

    let mut spans = vec![
        Span::styled(prompt.prefix().to_string(), emphasis_primary(model.interaction_mode)),
        Span::styled(prompt.input.clone(), emphasis_white()),
        Span::raw("  "),
    ];
    if let Some(error) = &prompt.error {
        spans.push(Span::styled(
            error.clone(),
            Style::default().fg(Color::Rgb(244, 86, 86)).add_modifier(Modifier::BOLD),
        ));
    } else {
        spans.push(Span::styled("enter apply  esc cancel", theme::muted()));
    }

    border_title_line(spans, model.interaction_mode, false)
}

fn set_prompt_cursor(frame: &mut Frame<'_>, area: Rect, model: &Model) {
    let Some(prompt) = model.prompt.as_ref() else {
        return;
    };

    let prefix = prompt.prefix().chars().count() as u16;
    let cursor = prompt.cursor.min(prompt.input.chars().count()) as u16;
    let x = area
        .x
        .saturating_add(1)
        .saturating_add(border_title_prefix_width())
        .saturating_add(prefix)
        .saturating_add(cursor);
    let y = area.y.saturating_add(area.height.saturating_sub(1));
    if x < area.x.saturating_add(area.width.saturating_sub(1)) {
        frame.set_cursor_position(Position { x, y });
    }
}

fn page_content_height(model: &Model) -> u16 {
    match model.page {
        Page::Amaru => screens::amaru_page_content_height(model),
        Page::Cardano => screens::cardano_page_content_height(model),
        Page::Config => screens::config_page_content_height(model),
    }
}

fn apply_shutdown_overlay(frame: &mut Frame<'_>, area: Rect) {
    let buffer = frame.buffer_mut();

    for y in area.y..area.y.saturating_add(area.height) {
        for x in area.x..area.x.saturating_add(area.width) {
            let Some(cell) = buffer.cell_mut((x, y)) else {
                continue;
            };
            cell.fg = grayscale_color(cell.fg);
            cell.bg = grayscale_color(cell.bg);
            cell.modifier.remove(Modifier::BOLD);
            cell.modifier.insert(Modifier::DIM);
        }
    }
}

fn grayscale_color(color: Color) -> Color {
    match color {
        Color::Reset => Color::Reset,
        Color::Black => Color::Black,
        Color::Red => grayscale_rgb(205, 49, 49),
        Color::Green => grayscale_rgb(13, 188, 121),
        Color::Yellow => grayscale_rgb(229, 229, 16),
        Color::Blue => grayscale_rgb(36, 114, 200),
        Color::Magenta => grayscale_rgb(188, 63, 188),
        Color::Cyan => grayscale_rgb(17, 168, 205),
        Color::Gray => grayscale_rgb(192, 192, 192),
        Color::DarkGray => grayscale_rgb(128, 128, 128),
        Color::LightRed => grayscale_rgb(241, 76, 76),
        Color::LightGreen => grayscale_rgb(35, 209, 139),
        Color::LightYellow => grayscale_rgb(245, 245, 67),
        Color::LightBlue => grayscale_rgb(59, 142, 234),
        Color::LightMagenta => grayscale_rgb(214, 112, 214),
        Color::LightCyan => grayscale_rgb(41, 184, 219),
        Color::White => grayscale_rgb(255, 255, 255),
        Color::Rgb(r, g, b) => grayscale_rgb(r, g, b),
        Color::Indexed(_) => Color::DarkGray,
    }
}

fn grayscale_rgb(r: u8, g: u8, b: u8) -> Color {
    let gray = ((299_u32 * r as u32 + 587_u32 * g as u32 + 114_u32 * b as u32) / 1000) as u8;
    Color::Rgb(gray, gray, gray)
}
