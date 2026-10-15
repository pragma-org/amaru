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
    layout::{Constraint, Rect},
    style::{Modifier, Style},
    widgets::{Block, Cell, Row, Table},
};

use super::super::{
    common::{
        border_title_chrome_width, border_title_line, border_title_prefix_width, button_label, format_vote_status,
        panel_borders, panel_padding, panel_title, render_scrollbar, scroll_panel_border, scroll_panel_border_type,
        table_body_area,
    },
    format::format_count,
    theme::{accent_primary, emphasis_primary, emphasis_white_color, striped_row_style, table_header_style},
};
use crate::{
    model::{InteractionMode, Model, ScrollFocus, TextPane, scrollbar::ScrollbarGeometry},
    ui::{
        Views,
        text_map::{column_offsets, push_plain_cell},
    },
};

pub(in crate::ui) fn render_proposals_table(frame: &mut Frame<'_>, area: Rect, model: &Model, views: &mut Views) {
    views.proposals_area = area;
    let proposals = model.proposals().collect::<Vec<_>>();
    let toggle_label = button_label(proposal_toggle_label(model));
    let focused = model.scroll_focus == ScrollFocus::Proposals;
    let title = panel_title(
        model.interaction_mode,
        focused,
        &format!(
            "Governance proposals ({})",
            model.governance.proposal_count_in_scope.map(format_count).unwrap_or_else(|| format_count(proposals.len()))
        ),
    );
    let block = Block::default()
        .title(title)
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
    views.proposals_body = body;
    let visible = body.height as usize;
    let proposals_len = proposals.len();
    let start = model.proposal_scroll.min(proposals_len.saturating_sub(visible));
    let columns = proposal_columns();
    let offsets = column_offsets(inner.width, &columns);
    record_proposal_header(&mut views.text_map, inner, &offsets);
    let rows = proposals
        .iter()
        .skip(start)
        .take(visible)
        .enumerate()
        .map(|(index, proposal)| {
            let y = body.y.saturating_add(u16::try_from(index).unwrap_or(u16::MAX));
            let plains = [
                proposal.id.clone(),
                proposal.kind.clone(),
                proposal.proposed_in.map(format_count).unwrap_or_else(|| "—".into()),
                proposal.valid_until.map(format_count).unwrap_or_else(|| "—".into()),
                format_vote_status(proposal.constitutional_committee).to_string(),
                format_vote_status(proposal.delegate_representatives).to_string(),
                format_vote_status(proposal.stake_pool_operators).to_string(),
                proposal.status.clone(),
            ];
            record_proposal_row(&mut views.text_map, inner.x, y, &offsets, &plains);
            let value = Style::default().fg(emphasis_white_color());
            let accent = Style::default().fg(accent_primary(model.interaction_mode));
            Row::new(vec![
                Cell::from(plains[0].clone()).style(value),
                Cell::from(plains[1].clone()).style(accent),
                Cell::from(plains[2].clone()).style(value),
                Cell::from(plains[3].clone()).style(value),
                Cell::from(plains[4].clone()).style(value),
                Cell::from(plains[5].clone()).style(value),
                Cell::from(plains[6].clone()).style(value),
                Cell::from(plains[7].clone()).style(accent.add_modifier(Modifier::BOLD)),
            ])
            .style(striped_row_style(start + index))
        })
        .collect::<Vec<_>>();
    views.proposal_toggle = Rect {
        x: area.x
            + area.width.saturating_sub(toggle_label.len() as u16 + border_title_chrome_width() + 1)
            + border_title_prefix_width(),
        y: area.y,
        width: toggle_label.len() as u16,
        height: 1,
    };
    let table = Table::new(rows, columns)
        .header(
            Row::new(vec!["Id", "Kind", "Proposed In", "Valid Until", "CC", "DRep", "SPO", "Status"])
                .style(table_header_style(model.interaction_mode)),
        )
        .column_spacing(1)
        .block(block);
    frame.render_widget(table, area);
    render_scrollbar(frame, body, proposals_len, visible, start, model.interaction_mode, false);
    note_scrollbar(&mut views.text_map, body, proposals_len, visible, start, model.interaction_mode);
}

fn proposal_columns() -> [Constraint; 8] {
    [
        Constraint::Min(42),
        Constraint::Length(26),
        Constraint::Length(13),
        Constraint::Length(13),
        Constraint::Length(4),
        Constraint::Length(6),
        Constraint::Length(5),
        Constraint::Length(10),
    ]
}

const PROPOSAL_HEADERS: [&str; 8] = ["Id", "Kind", "Proposed In", "Valid Until", "CC", "DRep", "SPO", "Status"];

fn record_proposal_header(map: &mut crate::ui::text_map::TextMap, inner: Rect, offsets: &[(u16, u16)]) {
    if inner.height == 0 {
        return;
    }
    let record = map.next_record();
    for (label, (dx, width)) in PROPOSAL_HEADERS.iter().zip(offsets) {
        push_plain_cell(map, inner.x.saturating_add(*dx), inner.y, *width, label, TextPane::Proposals, record);
    }
}

fn record_proposal_row(
    map: &mut crate::ui::text_map::TextMap,
    origin_x: u16,
    y: u16,
    offsets: &[(u16, u16)],
    plains: &[String; 8],
) {
    let record = map.next_record();
    for ((dx, width), plain) in offsets.iter().zip(plains) {
        push_plain_cell(map, origin_x.saturating_add(*dx), y, *width, plain, TextPane::Proposals, record);
    }
}

fn note_scrollbar(
    map: &mut crate::ui::text_map::TextMap,
    area: Rect,
    total: usize,
    visible: usize,
    position: usize,
    mode: InteractionMode,
) {
    if mode == InteractionMode::Copy || area.width == 0 || area.height == 0 {
        return;
    }
    if ScrollbarGeometry::new(total, visible, position, area.height as usize).is_some() {
        map.exclude(Rect { x: area.x + area.width.saturating_sub(1), y: area.y, width: 1, height: area.height });
    }
}

fn proposal_toggle_label(model: &Model) -> &'static str {
    if model.proposal_pane_mode.is_maximized() { "-" } else { "+" }
}
