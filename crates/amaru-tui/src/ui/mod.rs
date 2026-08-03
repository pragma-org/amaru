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
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders},
};

use self::{
    common::{
        accent_primary, border_title_chrome_width, border_title_line, border_title_prefix_width, button_label,
        spans_width, window_label,
    },
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
    if !is_ready {
        render_splash(frame, inner, model, views);
        return;
    }

    populate_shell_hotspots(views, shell_area, model);

    if model.page == Page::Amaru && model.peer_pane_mode.is_maximized() {
        render_peers_table(frame, inner, model, views);
        return;
    }

    if model.page == Page::Cardano && model.proposal_pane_mode.is_maximized() {
        render_proposals_table(frame, inner, model, views);
        return;
    }

    if model.log_pane_mode.is_maximized() && model.page != Page::Config {
        render_logs(frame, inner, model, views);
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
}

fn shell_block(model: &Model, is_ready: bool) -> Block<'static> {
    let block = Block::default().borders(Borders::ALL).border_style(border_primary(model.interaction_mode));

    if is_ready {
        block
            .title_top(page_tabs_line(model).left_aligned())
            .title_top(shell_title(model).centered())
            .title_top(window_controls_line(model).right_aligned())
            .title_bottom(shell_hint(model).right_aligned())
    } else {
        block.title_top(shell_title(model).centered())
    }
}

fn populate_shell_hotspots(views: &mut Views, area: Rect, model: &Model) {
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

    let labels = model.windows().iter().map(window_label).collect::<Vec<_>>();
    let total_width =
        spans_width(labels.iter().map(|label| label.len() as u16)) + labels.len().saturating_sub(1) as u16;
    let mut x =
        area.x + area.width.saturating_sub(total_width + border_title_chrome_width() + 1) + border_title_prefix_width();
    let y = area.y;

    for label in labels {
        views.window_tabs.push(Rect { x, y, width: label.len() as u16, height: 1 });
        x += label.len() as u16 + 1;
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
    if model.is_copy_mode() {
        return border_title_line(
            vec![
                Span::styled("<esc>", emphasis_primary(model.interaction_mode)),
                Span::styled(" NORMAL MODE", theme::muted()),
            ],
            model.interaction_mode,
            false,
        );
    }

    border_title_line(
        vec![
            Span::styled("<mouse>", emphasis_primary(model.interaction_mode)),
            Span::styled(" NAVIGATE  ", theme::muted()),
            Span::styled("<esc>", emphasis_primary(model.interaction_mode)),
            Span::styled(" COPY MODE  ", theme::muted()),
            Span::styled("<tab>", emphasis_primary(model.interaction_mode)),
            Span::styled(" NEXT  ", theme::muted()),
            Span::styled("<S-tab>", emphasis_primary(model.interaction_mode)),
            Span::styled(" PREV  ", theme::muted()),
            Span::styled("<←→>", emphasis_primary(model.interaction_mode)),
            Span::styled(" FOCUS  ", theme::muted()),
            Span::styled("<↑↓>", emphasis_primary(model.interaction_mode)),
            Span::styled(" SCROLL  ", theme::muted()),
            Span::styled("<enter>", emphasis_primary(model.interaction_mode)),
            Span::styled(" MAXIMIZE  ", theme::muted()),
            Span::styled("<q>", emphasis_primary(model.interaction_mode)),
            Span::styled(" QUIT", theme::muted()),
        ],
        model.interaction_mode,
        false,
    )
}

fn window_controls_line(model: &Model) -> Line<'static> {
    let mut spans = Vec::new();

    for (index, window) in model.windows().iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw(" "));
        }
        let style = if index == model.selected_window {
            emphasis_primary(model.interaction_mode)
        } else {
            Style::default().fg(Color::Rgb(210, 220, 235))
        };
        spans.push(Span::styled(window_label(window), style));
    }

    border_title_line(spans, model.interaction_mode, false)
}

fn page_content_height(model: &Model) -> u16 {
    match model.page {
        Page::Amaru => screens::amaru_page_content_height(model),
        Page::Cardano => screens::cardano_page_content_height(model),
        Page::Config => screens::config_page_content_height(model),
    }
}
