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
};

use super::super::{
    components::{render_card, render_peers_table, render_series_card},
    format::{
        aligned_pair_lines, format_count, format_density, format_duration, format_secs_frequency, format_slot_ratio,
    },
};
use crate::{model::Model, ui::Views};

pub(in crate::ui) fn render_amaru(frame: &mut Frame<'_>, area: Rect, model: &Model, views: &mut Views, now: Instant) {
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(7),
            Constraint::Length(6),
            Constraint::Length(peers_panel_height(model).max(mempool_panel_height())),
        ])
        .split(area);

    let charts = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(25),
            Constraint::Percentage(25),
            Constraint::Percentage(25),
            Constraint::Percentage(25),
        ])
        .split(layout[0]);
    render_series_card(
        frame,
        charts[0],
        "Memory (RSS)",
        sample_memory_mib(model),
        "MiB",
        memory_detail(model),
        model.interaction_mode,
    );
    render_series_card(frame, charts[1], "CPU", sample_cpu_tenths(model), "%", None, model.interaction_mode);
    render_series_card(
        frame,
        charts[2],
        "Disk Read",
        sample_disk_read_kib(model),
        "KiB/s",
        disk_read_detail(model),
        model.interaction_mode,
    );
    render_series_card(
        frame,
        charts[3],
        "Disk Write",
        sample_disk_write_kib(model),
        "KiB/s",
        disk_write_detail(model),
        model.interaction_mode,
    );

    let cards = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Fill(1), Constraint::Fill(1), Constraint::Fill(1), Constraint::Fill(1)])
        .split(layout[1]);

    render_card(
        frame,
        cards[0],
        "Node",
        aligned_pair_lines(vec![
            ("Network", model.startup.process.network.clone()),
            ("Platform", model.startup.process.target.clone()),
            ("Protocol", model.protocol_version.clone()),
            ("Uptime", format_duration(now.duration_since(model.created_at))),
        ]),
        model.interaction_mode,
    );

    let block_rate = blocks_per_second(model, now);
    let transaction_rate = transactions_per_second(model, now);
    render_card(
        frame,
        cards[1],
        "Throughput",
        aligned_pair_lines(vec![
            ("Blocks", format_count(model.blocks_in_window(now))),
            ("Blocks/s", format!("{block_rate:.2}")),
            ("Txs", format_count(model.transactions_in_window(now))),
            ("Tx/s", format!("{transaction_rate:.2}")),
        ]),
        model.interaction_mode,
    );

    if let Some(tip) = &model.tip {
        render_card(
            frame,
            cards[2],
            "Last Block",
            aligned_pair_lines(vec![
                ("Hash", tip.header_hash.chars().take(12).collect()),
                ("Slot", format_slot_ratio(tip.slot, model.startup.target_slot())),
                ("Height", format_count(tip.block_height)),
                (
                    "When",
                    model
                        .last_block_elapsed(now)
                        .map(|duration| format!("{} ago", format_duration(duration)))
                        .unwrap_or_else(|| "—".into()),
                ),
            ]),
            model.interaction_mode,
        );
    }

    render_card(
        frame,
        cards[3],
        "Chain quality",
        aligned_pair_lines(vec![
            ("Chain Growth", "-".into()),
            (
                "Chain Density",
                model
                    .tip
                    .as_ref()
                    .map(|tip| format_density(tip.density, model.startup.active_slot_coeff_inverse))
                    .unwrap_or_else(|| "—".into()),
            ),
            (
                "Rollback depth",
                model
                    .average_rollback_length(now)
                    .map(|value| if value == 1.0 { "~1 block".into() } else { format!("~{value:.1} blocks") })
                    .unwrap_or_else(|| "—".into()),
            ),
            ("Rollback freq.", model.rollback_frequency(now).map(format_secs_frequency).unwrap_or_else(|| "—".into())),
        ]),
        model.interaction_mode,
    );

    let bottom = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(78), Constraint::Percentage(22)])
        .split(layout[2]);
    render_peers_table(frame, bottom[0], model, views);
    render_card(
        frame,
        bottom[1],
        "Mempool",
        aligned_pair_lines(vec![
            ("Txs", format_count(model.mempool.tx_count)),
            ("Occupancy", format_kib_ratio(model.mempool.size_bytes, model.startup.mempool_max_bytes)),
        ]),
        model.interaction_mode,
    );
}

pub(in crate::ui) fn page_content_height(model: &Model) -> u16 {
    13 + peers_panel_height(model).max(mempool_panel_height())
}

fn peers_panel_height(model: &Model) -> u16 {
    3 + model.peers.len().min(10) as u16
}

fn mempool_panel_height() -> u16 {
    4
}

fn sample_memory_mib(model: &Model) -> Vec<u64> {
    model.system_samples.iter().map(|sample| sample.process_memory_bytes / 1_048_576).collect()
}

fn sample_cpu_tenths(model: &Model) -> Vec<u64> {
    model.system_samples.iter().map(|sample| sample.cpu_percent.round() as u64).collect()
}

fn sample_disk_read_kib(model: &Model) -> Vec<u64> {
    model.system_samples.iter().map(|sample| sample.disk_live_read_bytes.div_ceil(1_024)).collect()
}

fn sample_disk_write_kib(model: &Model) -> Vec<u64> {
    model.system_samples.iter().map(|sample| sample.disk_live_write_bytes.div_ceil(1_024)).collect()
}

fn blocks_per_second(model: &Model, now: Instant) -> f64 {
    let blocks = model.blocks_in_window(now) as f64;
    let seconds = model.current_window().as_secs_f64();
    if seconds == 0.0 { 0.0 } else { blocks / seconds }
}

fn transactions_per_second(model: &Model, now: Instant) -> f64 {
    let transactions = model.transactions_in_window(now) as f64;
    let seconds = model.current_window().as_secs_f64();
    if seconds == 0.0 { 0.0 } else { transactions / seconds }
}

fn memory_detail(model: &Model) -> Option<String> {
    let sample = model.system_samples.back()?;
    if sample.memory_total_bytes == 0 {
        return None;
    }

    let percentage = sample.process_memory_bytes as f64 / sample.memory_total_bytes as f64 * 100.0;
    Some(format!("{percentage:.1}%"))
}

fn disk_read_detail(model: &Model) -> Option<String> {
    let sample = model.system_samples.back()?;
    let total = sample.processes_live_read_bytes;
    if total == 0 {
        return Some("0.0%".into());
    }

    let share = sample.disk_live_read_bytes as f64 / total as f64 * 100.0;
    Some(format!("{share:.1}%"))
}

fn disk_write_detail(model: &Model) -> Option<String> {
    let sample = model.system_samples.back()?;
    let total = sample.processes_live_write_bytes;
    if total == 0 {
        return Some("0.0%".into());
    }

    let share = sample.disk_live_write_bytes as f64 / total as f64 * 100.0;
    Some(format!("{share:.1}%"))
}

fn format_kib(bytes: u64) -> String {
    format!("{} KiB", format_count(bytes.div_ceil(1_024)))
}

fn format_kib_ratio(bytes: u64, capacity_bytes: u64) -> String {
    format!("{} / {}", format_kib(bytes), format_kib(capacity_bytes))
}
