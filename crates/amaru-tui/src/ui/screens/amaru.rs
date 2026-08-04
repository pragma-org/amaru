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
    components::{render_card, render_gauge_card, render_peers_table},
    format::{
        aligned_pair_lines, format_count, format_density, format_duration, format_secs_frequency, format_slot_ratio,
    },
};
use crate::{events::SystemSample, model::Model, ui::Views};

pub(in crate::ui) fn render_amaru(frame: &mut Frame<'_>, area: Rect, model: &Model, views: &mut Views, now: Instant) {
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5),
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
    let sample = latest_system_sample(model);
    let memory = memory_gauge(sample);
    render_gauge_card(
        frame,
        charts[0],
        "Memory (RSS)",
        memory.label,
        memory.ratio,
        memory.detail,
        model.interaction_mode,
    );
    let cpu = cpu_gauge(sample);
    render_gauge_card(frame, charts[1], "CPU", cpu.label, cpu.ratio, cpu.detail, model.interaction_mode);
    let disk_read = disk_read_gauge(sample);
    render_gauge_card(
        frame,
        charts[2],
        "Disk Read",
        disk_read.label,
        disk_read.ratio,
        disk_read.detail,
        model.interaction_mode,
    );
    let disk_write = disk_write_gauge(sample);
    render_gauge_card(
        frame,
        charts[3],
        "Disk Write",
        disk_write.label,
        disk_write.ratio,
        disk_write.detail,
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
    render_peers_table(frame, bottom[0], model, views, now);
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
    11 + peers_panel_height(model).max(mempool_panel_height())
}

fn peers_panel_height(model: &Model) -> u16 {
    3 + model.peers.len().min(3) as u16
}

fn mempool_panel_height() -> u16 {
    4
}

fn latest_system_sample(model: &Model) -> Option<&SystemSample> {
    model.system_samples.back()
}

struct GaugeMetric {
    label: String,
    ratio: f64,
    detail: Option<String>,
}

fn memory_gauge(sample: Option<&SystemSample>) -> GaugeMetric {
    let Some(sample) = sample else {
        return GaugeMetric { label: "—".into(), ratio: 0.0, detail: None };
    };

    let current_mib = bytes_to_mib(sample.process_memory_bytes);
    let total_mib = bytes_to_mib(sample.memory_total_bytes);
    let ratio = linear_ratio(sample.process_memory_bytes, sample.memory_total_bytes);
    GaugeMetric {
        label: format!("{} / {} MiB", format_count(current_mib), format_count(total_mib)),
        ratio,
        detail: Some(format!("{:.1}%", ratio * 100.0)),
    }
}

fn cpu_gauge(sample: Option<&SystemSample>) -> GaugeMetric {
    let Some(sample) = sample else {
        return GaugeMetric { label: "—".into(), ratio: 0.0, detail: None };
    };

    GaugeMetric {
        label: format!("{:.1} / 100.0%", sample.cpu_percent),
        ratio: linear_ratio_f64(sample.cpu_percent, 100.0),
        detail: Some(format!("{:.1}%", sample.cpu_percent)),
    }
}

fn disk_read_gauge(sample: Option<&SystemSample>) -> GaugeMetric {
    disk_gauge(sample, |sample| sample.disk_live_read_bytes, |sample| sample.processes_live_read_bytes)
}

fn disk_write_gauge(sample: Option<&SystemSample>) -> GaugeMetric {
    disk_gauge(sample, |sample| sample.disk_live_write_bytes, |sample| sample.processes_live_write_bytes)
}

fn disk_gauge(
    sample: Option<&SystemSample>,
    current: impl Fn(&SystemSample) -> u64,
    total: impl Fn(&SystemSample) -> u64,
) -> GaugeMetric {
    let Some(sample) = sample else {
        return GaugeMetric { label: "—".into(), ratio: 0.0, detail: None };
    };

    let current = current(sample);
    let total = total(sample);
    let raw_ratio = linear_ratio(current, total);
    GaugeMetric {
        label: format!("{} / {} KiB/s", format_count(bytes_to_kib(current)), format_count(bytes_to_kib(total))),
        ratio: log_ratio(current, total),
        detail: Some(format!("{:.1}%", raw_ratio * 100.0)),
    }
}

fn bytes_to_mib(bytes: u64) -> u64 {
    bytes.div_ceil(1_048_576)
}

fn bytes_to_kib(bytes: u64) -> u64 {
    bytes.div_ceil(1_024)
}

fn linear_ratio(current: u64, max: u64) -> f64 {
    if max == 0 { 0.0 } else { (current as f64 / max as f64).clamp(0.0, 1.0) }
}

fn linear_ratio_f64(current: f64, max: f64) -> f64 {
    if max == 0.0 { 0.0 } else { (current / max).clamp(0.0, 1.0) }
}

fn log_ratio(current: u64, max: u64) -> f64 {
    if current == 0 || max == 0 { 0.0 } else { ((current as f64 + 1.0).ln() / (max as f64 + 1.0).ln()).clamp(0.0, 1.0) }
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

fn format_kib(bytes: u64) -> String {
    format!("{} KiB", format_count(bytes.div_ceil(1_024)))
}

fn format_kib_ratio(bytes: u64, capacity_bytes: u64) -> String {
    format!("{} / {}", format_kib(bytes), format_kib(capacity_bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_ratio_keeps_bounds() {
        assert_eq!(log_ratio(0, 1_000), 0.0);
        assert_eq!(log_ratio(1_000, 1_000), 1.0);
        assert!(log_ratio(1, 1_000) > linear_ratio(1, 1_000));
    }

    #[test]
    fn memory_gauge_uses_process_footprint_against_total_memory() {
        let sample = SystemSample {
            at: Instant::now(),
            cpu_percent: 0.0,
            process_memory_bytes: 512 * 1_048_576,
            rss_bytes: 0,
            virtual_bytes: 0,
            memory_used_bytes: 0,
            memory_total_bytes: 2 * 1_048_576 * 1_024,
            disk_read_bytes: 0,
            disk_write_bytes: 0,
            disk_live_read_bytes: 0,
            disk_live_write_bytes: 0,
            processes_live_read_bytes: 0,
            processes_live_write_bytes: 0,
        };

        let gauge = memory_gauge(Some(&sample));

        assert_eq!(gauge.label, "512 / 2,048 MiB");
        assert_eq!(gauge.detail.as_deref(), Some("25.0%"));
        assert_eq!(gauge.ratio, 0.25);
    }
}
