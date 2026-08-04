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
    widgets::{Block, Borders, Cell, Row, Table},
};

use super::super::{
    common::{
        border_title_chrome_width, border_title_line, border_title_prefix_width, button_label, format_vote_status,
        panel_title, render_scrollbar, scroll_panel_border, scroll_panel_border_type, table_body_area,
    },
    format::format_count,
    theme::{accent_primary, emphasis_primary, emphasis_white_color, striped_row_style, table_header_style},
};
use crate::{
    model::{Model, ScrollFocus},
    ui::Views,
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
        .borders(Borders::ALL)
        .border_style(scroll_panel_border(focused, model.interaction_mode))
        .border_type(scroll_panel_border_type(focused));
    let inner = block.inner(area);
    let body = table_body_area(inner);
    let visible = body.height as usize;
    let proposals_len = proposals.len();
    let start = model.proposal_scroll.min(proposals_len.saturating_sub(visible));
    let rows = proposals
        .iter()
        .skip(start)
        .take(visible)
        .enumerate()
        .map(|(index, proposal)| {
            Row::new(vec![
                Cell::from(proposal.id.clone()).style(Style::default().fg(emphasis_white_color())),
                Cell::from(proposal.kind.clone()).style(Style::default().fg(accent_primary(model.interaction_mode))),
                Cell::from(proposal.proposed_in.map(format_count).unwrap_or_else(|| "—".into()))
                    .style(Style::default().fg(emphasis_white_color())),
                Cell::from(proposal.valid_until.map(format_count).unwrap_or_else(|| "—".into()))
                    .style(Style::default().fg(emphasis_white_color())),
                Cell::from(format_vote_status(proposal.constitutional_committee))
                    .style(Style::default().fg(emphasis_white_color())),
                Cell::from(format_vote_status(proposal.delegate_representatives))
                    .style(Style::default().fg(emphasis_white_color())),
                Cell::from(format_vote_status(proposal.stake_pool_operators))
                    .style(Style::default().fg(emphasis_white_color())),
                Cell::from(proposal.status.clone())
                    .style(Style::default().fg(accent_primary(model.interaction_mode)).add_modifier(Modifier::BOLD)),
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
    let table = Table::new(
        rows,
        [
            Constraint::Min(42),
            Constraint::Length(26),
            Constraint::Length(13),
            Constraint::Length(13),
            Constraint::Length(4),
            Constraint::Length(6),
            Constraint::Length(5),
            Constraint::Length(10),
        ],
    )
    .header(
        Row::new(vec!["Id", "Kind", "Proposed In", "Valid Until", "CC", "DRep", "SPO", "Status"])
            .style(table_header_style(model.interaction_mode)),
    )
    .column_spacing(1)
    .block(block);
    frame.render_widget(table, area);
    render_scrollbar(frame, body, proposals_len, visible, start, model.interaction_mode);
}

fn proposal_toggle_label(model: &Model) -> &'static str {
    if model.proposal_pane_mode.is_maximized() { "-" } else { "+" }
}
