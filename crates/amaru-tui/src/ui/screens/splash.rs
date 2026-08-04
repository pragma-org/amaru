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
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

use super::super::{
    common::{amaru_gradient_color, centered_rect, render_gradient_progress_bar},
    format::format_count,
    theme::{block_title, border_secondary},
};
use crate::{
    model::{InitialStakeDistributionState, InteractionMode, Model},
    ui::Views,
};

pub(in crate::ui) fn render_splash(frame: &mut Frame<'_>, area: Rect, model: &Model, _views: &mut Views) {
    let progress_states = model.initial_stake_distributions().collect::<Vec<_>>();
    let progress_height = splash_progress_height(progress_states.len());
    let logo_area =
        Rect { x: area.x, y: area.y, width: area.width, height: area.height.saturating_sub(progress_height) };
    let logo = splash_logo(logo_area, model.interaction_mode);
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Fill(1),
            Constraint::Length(logo.height),
            Constraint::Fill(1),
            Constraint::Length(progress_height),
            Constraint::Fill(1),
        ])
        .split(area);
    let logo_popup = centered_rect(layout[1], logo.width, logo.height);
    let paragraph = Paragraph::new(logo.lines).alignment(ratatui::layout::Alignment::Center).wrap(Wrap { trim: false });

    frame.render_widget(Clear, area);
    frame.render_widget(paragraph, logo_popup);
    render_splash_progress(frame, layout[3], model, &progress_states);
}

const LOGO_SHAPES: &[&[(f32, f32)]] = &[
    &[(814.326, 745.646), (822.949, 654.382), (833.843, 661.216), (834.603, 759.386)],
    &[(776.692, 764.223), (803.084, 623.251), (816.716, 633.17), (802.478, 783.86)],
    &[(689.882, 747.099), (768.258, 580.04), (799.003, 600.275), (760.908, 803.755)],
];

struct SplashLogo {
    width: u16,
    height: u16,
    lines: Vec<Line<'static>>,
}

fn splash_progress_slots(progress_count: usize) -> usize {
    progress_count.max(2)
}

fn splash_progress_height(progress_count: usize) -> u16 {
    let slots = splash_progress_slots(progress_count);
    (slots as u16).saturating_mul(2).saturating_add(1)
}

fn render_splash_progress(
    frame: &mut Frame<'_>,
    area: Rect,
    model: &Model,
    progress_states: &[&InitialStakeDistributionState],
) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let block_width = area.width.clamp(24, 76);
    let block_area = Rect {
        x: area.x + area.width.saturating_sub(block_width) / 2,
        y: area.y,
        width: block_width,
        height: area.height,
    };
    let block = Block::default()
        .title(block_title(model.interaction_mode, "Loading initial stake distributions..."))
        .borders(Borders::ALL)
        .border_style(border_secondary(model.interaction_mode));
    let inner = block.inner(block_area);
    let gauge_width = inner.width;
    let mut y = inner.y;
    let display_count = splash_progress_slots(progress_states.len());

    frame.render_widget(block, block_area);

    for index in 0..display_count {
        let (ratio, label) = if let Some(state) = progress_states.get(index) {
            (
                state.progress.clamp(0.0, 1.0),
                format!("Epoch {} ({:.0}%)", format_count(state.epoch), state.progress * 100.0),
            )
        } else {
            (0.0, "Pending...".to_string())
        };

        render_gradient_progress_bar(frame, Rect { x: inner.x, y, width: gauge_width, height: 1 }, ratio, &label);
        y += 2;
    }
}

fn splash_logo(area: Rect, mode: InteractionMode) -> SplashLogo {
    let ((min_x, min_y), (max_x, max_y)) = logo_bounds();
    let aspect = (max_x - min_x) / (max_y - min_y);
    let max_height = area.height.saturating_mul(4) / 5;

    let mut height = max_height.max(1);
    let mut width = ((height as f32) * aspect * 2.0).round() as u16;

    if width > area.width {
        width = area.width.max(1);
        height = ((width as f32) / (aspect * 2.0)).round() as u16;
    }

    width = width.clamp(1, area.width.max(1));
    height = height.clamp(1, area.height.max(1));

    let lines = (0..height)
        .map(|row| {
            let mut spans = Vec::new();
            let mut run = String::new();
            let mut current_style: Option<Style> = None;

            for column in 0..width {
                let glyph = quadrant_glyph(sample_logo_cell(column, row, width, height, min_x, min_y, max_x, max_y));
                let style = splash_logo_style(mode, column, width, glyph);

                if current_style == Some(style) {
                    run.push(glyph);
                } else {
                    if !run.is_empty() {
                        spans.push(Span::styled(std::mem::take(&mut run), current_style.unwrap_or_default()));
                    }
                    run.push(glyph);
                    current_style = Some(style);
                }
            }

            if !run.is_empty() {
                spans.push(Span::styled(run, current_style.unwrap_or_default()));
            }

            Line::from(spans)
        })
        .collect();

    SplashLogo { width, height, lines }
}

fn logo_bounds() -> ((f32, f32), (f32, f32)) {
    let mut min_x = f32::MAX;
    let mut min_y = f32::MAX;
    let mut max_x = f32::MIN;
    let mut max_y = f32::MIN;

    for shape in LOGO_SHAPES {
        for (x, y) in *shape {
            min_x = min_x.min(*x);
            min_y = min_y.min(*y);
            max_x = max_x.max(*x);
            max_y = max_y.max(*y);
        }
    }

    ((min_x, min_y), (max_x, max_y))
}

#[allow(clippy::too_many_arguments)]
fn sample_logo_cell(
    column: u16,
    row: u16,
    width: u16,
    height: u16,
    min_x: f32,
    min_y: f32,
    max_x: f32,
    max_y: f32,
) -> u8 {
    let mut bits = 0_u8;
    let subpixel_width = (width as f32) * 2.0;
    let subpixel_height = (height as f32) * 2.0;
    let logo_width = max_x - min_x;
    let logo_height = max_y - min_y;

    for (bit, sub_x, sub_y) in [(0_u8, 0_u16, 0_u16), (1_u8, 1, 0), (2_u8, 0, 1), (3_u8, 1, 1)] {
        let x = ((((column * 2) + sub_x) as f32) + 0.5) / subpixel_width;
        let y = ((((row * 2) + sub_y) as f32) + 0.5) / subpixel_height;
        let sample = (min_x + (x * logo_width), min_y + (y * logo_height));

        if LOGO_SHAPES.iter().any(|shape| point_in_polygon(sample, shape)) {
            bits |= 1 << bit;
        }
    }

    bits
}

fn point_in_polygon(point: (f32, f32), polygon: &[(f32, f32)]) -> bool {
    let (x, y) = point;
    let mut inside = false;
    let mut previous = polygon[polygon.len() - 1];

    for &current in polygon {
        let (xi, yi) = current;
        let (xj, yj) = previous;
        let intersects = ((yi > y) != (yj > y)) && (x < ((xj - xi) * (y - yi) / ((yj - yi) + f32::EPSILON)) + xi);
        if intersects {
            inside = !inside;
        }
        previous = current;
    }

    inside
}

fn quadrant_glyph(bits: u8) -> char {
    match bits {
        0 => ' ',
        1 => '▘',
        2 => '▝',
        3 => '▀',
        4 => '▖',
        5 => '▌',
        6 => '▞',
        7 => '▛',
        8 => '▗',
        9 => '▚',
        10 => '▐',
        11 => '▜',
        12 => '▄',
        13 => '▙',
        14 => '▟',
        15 => '█',
        _ => ' ',
    }
}

fn splash_logo_style(_mode: InteractionMode, column: u16, width: u16, glyph: char) -> Style {
    if glyph == ' ' {
        return Style::default();
    }

    let x = if width <= 1 { 0.0 } else { column as f32 / (width - 1) as f32 };
    Style::default().fg(amaru_gradient_color(x))
}
