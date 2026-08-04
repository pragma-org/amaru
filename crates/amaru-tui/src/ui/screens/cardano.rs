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
    text::{Line, Span},
};

use super::super::{
    components::{render_card, render_proposals_table},
    format::{aligned_pair_lines, format_count, format_lovelace},
    theme::muted,
};
use crate::{model::Model, ui::Views};

pub(in crate::ui) fn render_cardano(
    frame: &mut Frame<'_>,
    area: Rect,
    model: &Model,
    views: &mut Views,
    _now: Instant,
) {
    if model.proposal_pane_mode.is_maximized() {
        render_proposals_table(frame, area, model, views);
        return;
    }

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(7), Constraint::Min(proposals_panel_height(model))])
        .split(area);

    let cards = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Fill(1), Constraint::Fill(1), Constraint::Fill(1), Constraint::Fill(1)])
        .split(layout[0]);

    render_card(
        frame,
        cards[0],
        "Protocol",
        aligned_pair_lines(vec![
            ("Epoch", model.tip.as_ref().map(|tip| format_count(tip.epoch)).unwrap_or_else(|| "—".into())),
            ("Slot", model.tip.as_ref().map(|tip| format_count(tip.slot)).unwrap_or_else(|| "—".into())),
            ("Version", model.protocol_version.clone()),
        ]),
        model.interaction_mode,
    );

    render_card(
        frame,
        cards[1],
        "Money Pots",
        aligned_pair_lines(vec![
            ("Treasury", model.treasury.map(format_lovelace).unwrap_or_else(|| "—".into())),
            ("Reserves", model.reserves.map(format_lovelace).unwrap_or_else(|| "—".into())),
            ("Fees", model.fees.map(format_lovelace).unwrap_or_else(|| "—".into())),
            ("Donations", format_lovelace(model.donations.unwrap_or(0))),
        ]),
        model.interaction_mode,
    );

    render_card(
        frame,
        cards[2],
        "Internal state",
        aligned_pair_lines(vec![
            ("Epoch overlay", if model.epoch_overlay_exists { "✓".into() } else { "—".into() }),
            ("Rewards", if model.rewards_ready { "computed".into() } else { "not ready".into() }),
        ]),
        model.interaction_mode,
    );

    let stake_lines = if let Some(snapshot) = &model.stake_snapshot {
        aligned_pair_lines(vec![
            ("Accounts", format_count(snapshot.accounts)),
            ("Pools", format_count(snapshot.pools)),
            ("DReps", format_count(snapshot.dreps)),
            ("Active", format_stake_distribution(snapshot.active_stake, model.startup.max_lovelace_supply)),
        ])
    } else {
        vec![Line::from(Span::styled("No stake snapshot telemetry yet", muted()))]
    };
    render_card(frame, cards[3], "Stake distribution", stake_lines, model.interaction_mode);
    render_proposals_table(frame, layout[1], model, views);
}

pub(in crate::ui) fn page_content_height(model: &Model) -> u16 {
    7 + proposals_panel_height(model)
}

fn proposals_panel_height(model: &Model) -> u16 {
    model.proposal_order.len().clamp(4, 10).saturating_add(3) as u16
}

fn format_stake_distribution(active_stake: u64, max_lovelace_supply: u64) -> String {
    let percentage =
        if max_lovelace_supply == 0 { 0.0 } else { active_stake as f64 / max_lovelace_supply as f64 * 100.0 };
    format!("{} ({percentage:.1}%)", format_lovelace(active_stake))
}
