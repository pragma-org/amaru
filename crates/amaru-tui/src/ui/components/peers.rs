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
    layout::{Constraint, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Row, Table},
};

use super::super::{
    common::{
        border_title_chrome_width, border_title_line, border_title_prefix_width, button_label, panel_title,
        render_scrollbar, scroll_panel_border, scroll_panel_border_type, table_body_area,
    },
    format::{format_count, format_micros},
    theme::{
        accent_primary, emphasis_primary, emphasis_white_color, muted_color, striped_row_style, table_header_style,
    },
};
use crate::{
    model::{InteractionMode, Model, PeerState, ScrollFocus},
    ui::Views,
};

pub(in crate::ui) fn render_peers_table(
    frame: &mut Frame<'_>,
    area: Rect,
    model: &Model,
    views: &mut Views,
    _now: Instant,
) {
    views.peers_area = area;
    let focused = model.scroll_focus == ScrollFocus::Peers;
    let toggle_label = button_label(peer_toggle_label(model));
    let block = Block::default()
        .title(panel_title(model.interaction_mode, focused, &format!("Peers ({})", format_count(model.peers.len()))))
        .title_top(
            border_title_line(
                vec![ratatui::text::Span::styled(toggle_label.clone(), emphasis_primary(model.interaction_mode))],
                model.interaction_mode,
                focused,
            )
            .right_aligned(),
        )
        .borders(Borders::ALL)
        .border_style(scroll_panel_border(focused, model.interaction_mode))
        .border_type(scroll_panel_border_type(focused));
    let inner = block.inner(area);
    let body = table_body_area(inner);
    let peers = model.sorted_peers();
    let visible = body.height as usize;
    let start = model.peer_scroll.min(peers.len().saturating_sub(visible));
    let rows = peers
        .into_iter()
        .skip(start)
        .take(visible)
        .enumerate()
        .map(|(index, peer)| peer_row(start + index, peer, model.interaction_mode))
        .collect::<Vec<_>>();
    views.peer_toggle = Rect {
        x: area.x
            + area.width.saturating_sub(toggle_label.len() as u16 + border_title_chrome_width() + 1)
            + border_title_prefix_width(),
        y: area.y,
        width: toggle_label.len() as u16,
        height: 1,
    };
    let table = Table::new(
        rows,
        [
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Fill(4),
            Constraint::Fill(1),
            Constraint::Fill(1),
            Constraint::Fill(1),
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(1),
            Constraint::Fill(1),
        ],
    )
    .header(
        Row::new(vec!["", "Dir", "Peer", "Duplex?", "RTT", "Observe", "→", "Select", "→", "Fetch", "→", "Adopt"])
            .style(table_header_style(model.interaction_mode)),
    )
    .column_spacing(1)
    .block(block);
    frame.render_widget(table, area);
    render_scrollbar(frame, body, model.peers.len(), visible, start, model.interaction_mode);
}

fn peer_toggle_label(model: &Model) -> &'static str {
    if model.peer_pane_mode.is_maximized() { "-" } else { "+" }
}

fn peer_row(index: usize, peer: &PeerState, mode: InteractionMode) -> Row<'static> {
    let direction = if peer.full_duplex == Some(true) {
        "↕"
    } else {
        match (peer.inbound, peer.outbound) {
            (true, true) => "▲▼",
            (true, false) => "▲",
            (false, true) => "▼",
            (false, false) => "-",
        }
    };
    let state_dot = " ●";
    let rtt =
        peer.last_rtt_micros.map(|value| format!("{:.1} ms", value as f64 / 1_000.0)).unwrap_or_else(|| "—".into());
    let slot_start_to_header = peer.mean_slot_start_to_header_micros().map(format_micros).unwrap_or_else(|| "—".into());
    let query_header = peer.mean_query_header_micros().map(format_micros).unwrap_or_else(|| "—".into());
    let get_block = peer.mean_get_block_micros().map(format_micros).unwrap_or_else(|| "—".into());
    let adopt_block = peer.mean_adopt_block_micros().map(format_micros).unwrap_or_else(|| "—".into());
    let can_duplex = match peer.full_duplex_capable {
        Some(true) => "yes",
        Some(false) => "no",
        None => "—",
    };

    Row::new(vec![
        Cell::from(state_dot).style(Style::default().fg(if peer.connected {
            accent_primary(mode)
        } else {
            Color::Rgb(244, 86, 86)
        })),
        Cell::from(direction).style(Style::default().fg(accent_primary(mode))),
        Cell::from(peer_address_line(peer)),
        Cell::from(can_duplex).style(Style::default().fg(muted_color())),
        Cell::from(rtt).style(Style::default().fg(emphasis_white_color())),
        Cell::from(slot_start_to_header).style(Style::default().fg(emphasis_white_color())),
        Cell::from("→"),
        Cell::from(query_header).style(Style::default().fg(emphasis_white_color())),
        Cell::from("→"),
        Cell::from(get_block).style(Style::default().fg(emphasis_white_color())),
        Cell::from("→"),
        Cell::from(adopt_block).style(Style::default().fg(emphasis_white_color())),
    ])
    .style(striped_row_style(index))
}

fn peer_address_line(peer: &PeerState) -> Line<'static> {
    let address = Span::styled(peer.address.clone(), Style::default().fg(emphasis_white_color()));
    match peer.candidate_label() {
        Some(candidate) => {
            Line::from(vec![address, Span::styled(format!(" ({candidate})"), Style::default().fg(muted_color()))])
        }
        None => Line::from(address),
    }
}
