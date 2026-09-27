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
    widgets::{Block, Cell, Row, Table},
};

use super::super::{
    common::{
        border_title_chrome_width, border_title_line, border_title_prefix_width, button_label, panel_borders,
        panel_padding, panel_title, render_scrollbar, scroll_panel_border, scroll_panel_border_type, table_body_area,
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
        .borders(panel_borders(model.interaction_mode))
        .border_style(scroll_panel_border(focused, model.interaction_mode))
        .border_type(scroll_panel_border_type(focused))
        .padding(panel_padding(model.interaction_mode));
    let inner = block.inner(area);
    let body = table_body_area(inner);
    views.peers_body = body;
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
            Constraint::Fill(1),
            Constraint::Fill(1),
            Constraint::Fill(1),
        ],
    )
    .header(
        Row::new(vec![
            "", "Dir", "Peer", "Duplex?", "RTT", "Observe", "→", "Select", "→", "Fetch", "→", "Adopt", "≤1s", "≤3s",
            "≤5s",
        ])
        .style(table_header_style(model.interaction_mode)),
    )
    .column_spacing(1)
    .block(block);
    frame.render_widget(table, area);
    render_scrollbar(frame, body, model.peers.len(), visible, start, model.interaction_mode, false);
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
    let within_1s = format_share(peer.live_arrival_share_percent(1_000_000));
    let within_3s = format_share(peer.live_arrival_share_percent(3_000_000));
    let within_5s = format_share(peer.live_arrival_share_percent(5_000_000));
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
        Cell::from(within_1s).style(Style::default().fg(emphasis_white_color())),
        Cell::from(within_3s).style(Style::default().fg(emphasis_white_color())),
        Cell::from(within_5s).style(Style::default().fg(emphasis_white_color())),
    ])
    .style(striped_row_style(index))
}

fn format_share(percent: Option<u64>) -> String {
    percent.map(|percent| format!("{percent}%")).unwrap_or_else(|| "—".into())
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

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};

    use super::*;
    use crate::{
        config::Config,
        model::{Model, PeerState},
        startup::{ProcessInfo, StartupContext},
        ui::Views,
    };

    fn fixture_startup_context() -> StartupContext {
        StartupContext {
            process: ProcessInfo {
                pid: 42,
                network: "preview".into(),
                software_version: "10.11.0 (abc123)".into(),
                target: "darwin/aarch64".into(),
            },
            protocol_version: "10.11".into(),
            mempool_max_bytes: 180_224,
            epoch_length: 86_400,
            active_slot_coeff_inverse: 20,
            consensus_security_param: 432,
            max_lovelace_supply: 45_000_000_000_000_000,
            system_start_millis: 1_666_656_000_000,
            era_history: None,
            runtime_sections: Vec::default(),
            protocol_sections: Vec::default(),
        }
    }

    fn buffer_lines(buffer: &Buffer) -> Vec<String> {
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer.cell((x, y)).map(|cell| cell.symbol()).unwrap_or("").to_string())
                    .collect()
            })
            .collect()
    }

    #[test]
    fn peer_row_shows_timing_and_arrival_share_cells() {
        let mut model = Model::new(Config::default(), fixture_startup_context());
        let at = Instant::now();
        let mut peer = PeerState::new("1.2.3.4:3001".into(), at);
        peer.record_header_lifecycle(at, 100, Some(9_000), Some(2_000), Some(5_000), Some(8_000));
        peer.record_live_arrival(500_000, 100);
        peer.record_live_arrival(2_000_000, 100);
        peer.record_live_arrival(4_000_000, 100);
        model.peers.insert(peer.address.clone(), peer);

        let backend = TestBackend::new(160, 8);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut views = Views::default();
        terminal
            .draw(|frame| render_peers_table(frame, frame.area(), &model, &mut views, at))
            .expect("draw peer table");
        let lines = buffer_lines(terminal.backend().buffer());

        let header = lines.iter().find(|line| line.contains("Observe")).expect("header row");
        for label in ["Select", "Fetch", "Adopt", "≤1s", "≤3s", "≤5s"] {
            assert!(header.contains(label), "header missing {label}: {header}");
        }

        let row = lines.iter().find(|line| line.contains("1.2.3.4:3001")).expect("peer row");
        for cell in ["9.0ms", "2.0ms", "5.0ms", "8.0ms", "33%", "67%", "100%"] {
            assert!(row.contains(cell), "peer row missing {cell}: {row}");
        }
    }
}
