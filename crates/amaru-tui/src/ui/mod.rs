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
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Cell, Clear, Gauge, Paragraph, Row, Scrollbar, ScrollbarOrientation, ScrollbarState, Sparkline,
        Table, Wrap,
    },
};

use self::{
    format::{
        aligned_pair_lines, format_count, format_density, format_duration, format_log_wall_time, format_lovelace,
        format_ratio, format_slot_ratio, target_slot,
    },
    theme::{
        accent_primary, block_title, border_primary, border_secondary, emphasis_primary, emphasis_white,
        emphasis_white_color, muted, muted_color, striped_row_style, style_for_level, style_for_level_filter,
        style_for_target, table_header_style,
    },
};
use crate::{
    config::format_duration_short,
    model::{LevelFilter, Model, Page, PeerState, TargetFilter},
    startup::ConfigSection,
};

mod format;
mod theme;

#[derive(Debug, Default, Clone)]
pub struct Hotspots {
    pub page_tabs: Vec<(Page, Rect)>,
    pub log_toggle: Rect,
    pub window_tabs: Vec<Rect>,
    pub level_tabs: Vec<(LevelFilter, Rect)>,
    pub target_tabs: Vec<(TargetFilter, Rect)>,
    pub logs_area: Rect,
    pub peers_area: Rect,
    pub proposals_area: Rect,
}

pub fn render(frame: &mut Frame<'_>, model: &Model, hotspots: &mut Hotspots, now: Instant) {
    hotspots.page_tabs.clear();
    hotspots.window_tabs.clear();
    hotspots.level_tabs.clear();
    hotspots.target_tabs.clear();
    hotspots.log_toggle = Rect::default();
    hotspots.logs_area = Rect::default();
    hotspots.peers_area = Rect::default();
    hotspots.proposals_area = Rect::default();

    let progress_height = u16::from(model.tip.is_some()) * 2;
    let shell = shell_block(model);
    let shell_area = frame.area();
    let inner = shell.inner(shell_area);

    frame.render_widget(shell, shell_area);
    populate_shell_hotspots(hotspots, shell_area, model);

    if model.log_pane_mode.is_maximized() && model.page != Page::Config {
        render_logs(frame, inner, model, hotspots);
        return;
    }

    let available_height = inner.height.saturating_sub(progress_height);
    let show_logs = model.page != Page::Config;

    if show_logs {
        let max_content_height = available_height.saturating_sub(10);
        let desired_content_height = if model.is_ready(now) { page_content_height(model) } else { max_content_height };
        let content_height = desired_content_height.min(max_content_height);
        let log_height = available_height.saturating_sub(content_height);
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(content_height),
                Constraint::Length(log_height),
                Constraint::Length(progress_height),
            ])
            .split(inner);

        if model.is_ready(now) {
            match model.page {
                Page::Amaru => render_amaru(frame, layout[0], model, hotspots, now),
                Page::Cardano => render_cardano(frame, layout[0], model, hotspots, now),
                Page::Config => render_config(frame, layout[0], model),
            }
        } else {
            render_splash(frame, layout[0], model);
        }

        render_logs(frame, layout[1], model, hotspots);

        if progress_height > 0 {
            render_epoch_progress(frame, layout[2], model);
        }
    } else {
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(available_height), Constraint::Length(progress_height)])
            .split(inner);

        if model.is_ready(now) {
            render_config(frame, layout[0], model);
        } else {
            render_splash(frame, layout[0], model);
        }

        if progress_height > 0 {
            render_epoch_progress(frame, layout[1], model);
        }
    }
}

fn shell_block(model: &Model) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(border_primary())
        .title_top(page_tabs_line(model).left_aligned())
        .title_top(shell_title(model).centered())
        .title_top(window_controls_line(model).right_aligned())
        .title_bottom(shell_hint().left_aligned())
}

fn populate_shell_hotspots(hotspots: &mut Hotspots, area: Rect, model: &Model) {
    let mut x = area.x + 2 + border_title_prefix_width();
    let y = area.y;
    for (index, page) in Page::ALL.into_iter().enumerate() {
        let label = button_label(page.label());
        hotspots.page_tabs.push((page, Rect { x, y, width: label.len() as u16, height: 1 }));
        x += label.len() as u16;
        if index + 1 != Page::ALL.len() {
            x += 1;
        }
    }

    let labels = model.windows().iter().map(|window| button_label(&format_duration_short(*window))).collect::<Vec<_>>();
    let total_width =
        spans_width(labels.iter().map(|label| label.len() as u16)) + labels.len().saturating_sub(1) as u16;
    let mut x =
        area.x + area.width.saturating_sub(total_width + border_title_chrome_width() + 1) + border_title_prefix_width();
    let y = area.y;

    for label in labels {
        hotspots.window_tabs.push(Rect { x, y, width: label.len() as u16, height: 1 });
        x += label.len() as u16 + 1;
    }
}

fn render_splash(frame: &mut Frame<'_>, area: Rect, _model: &Model) {
    let lines = vec![
        splash_logo_line(18),
        splash_logo_line(26),
        splash_logo_line(34),
        Line::from(Span::raw("")),
        Line::from(Span::styled("AMARU", emphasis_primary())),
        Line::from(Span::raw("")),
        Line::from(Span::styled("Loading initial stake distributions...", muted().add_modifier(Modifier::ITALIC))),
    ];

    let content_height = lines.len() as u16 + 2;
    let popup = centered_rect(area, 68, content_height);
    let paragraph = Paragraph::new(lines)
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: false })
        .block(Block::default().borders(Borders::ALL).border_style(border_primary()));
    frame.render_widget(Clear, popup);
    frame.render_widget(paragraph, popup);
}

fn render_amaru(frame: &mut Frame<'_>, area: Rect, model: &Model, hotspots: &mut Hotspots, now: Instant) {
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(7), Constraint::Length(7), Constraint::Min(peers_panel_height(model))])
        .split(area);

    let cards = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(23),
            Constraint::Percentage(37),
            Constraint::Percentage(18),
            Constraint::Percentage(20),
        ])
        .split(layout[0]);

    render_card(
        frame,
        cards[0],
        "Node",
        aligned_pair_lines(vec![
            ("Version", model.startup.process.software_version.clone()),
            ("Target", model.startup.process.target.clone()),
            ("Peers", format_count(model.connected_peer_count())),
            ("Trusted", format_count(model.startup.trusted_peers.len())),
        ]),
    );

    let tip_lines = if let Some(tip) = &model.tip {
        aligned_pair_lines(vec![
            ("Epoch", format_count(tip.epoch)),
            ("Slot", format_slot_ratio(tip.slot, target_slot(model.startup.system_start_millis))),
            ("Height", format_count(tip.block_height)),
            ("Hash", elide(&tip.header_hash, 40)),
            ("Density", format_density(tip.density, model.startup.active_slot_coeff_inverse)),
        ])
    } else {
        vec![Line::from(Span::styled("No tip telemetry yet", muted()))]
    };
    render_card(frame, cards[1], "Tip", tip_lines);

    let block_rate = blocks_per_second(model, now);
    let transaction_rate = transactions_per_second(model, now);
    render_card(
        frame,
        cards[2],
        "Throughput",
        aligned_pair_lines(vec![
            ("Window", model.window_label()),
            ("Blocks", format_count(model.blocks_in_window(now))),
            ("Txs", format_count(model.transactions_in_window(now))),
            ("Blocks/s", format!("{block_rate:.2}")),
            ("Tx/s", format!("{transaction_rate:.2}")),
        ]),
    );

    let peer_lines = aligned_pair_lines(vec![
        ("Inbound", format_count(model.inbound_peer_count())),
        ("Outbound", format_count(model.outbound_peer_count())),
        ("RTT", model.average_rtt_millis().map(|value| format!("{value:.1} ms")).unwrap_or_else(|| "—".into())),
        (
            "Last block",
            model
                .last_block_elapsed(now)
                .map(|duration| format!("{} ago", format_duration(duration)))
                .unwrap_or_else(|| "—".into()),
        ),
    ]);
    render_card(frame, cards[3], "Network health", peer_lines);

    let charts = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(34), Constraint::Percentage(33), Constraint::Percentage(33)])
        .split(layout[1]);
    render_series_card(frame, charts[0], "Memory", sample_memory_mib(model), "MiB");
    render_series_card(frame, charts[1], "CPU", sample_cpu_tenths(model), "%");
    render_series_card(frame, charts[2], "Disk I/O", sample_disk_kib(model), "KiB/s");

    render_peers_table(frame, layout[2], model, hotspots);
}

fn render_cardano(frame: &mut Frame<'_>, area: Rect, model: &Model, hotspots: &mut Hotspots, _now: Instant) {
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(7), Constraint::Min(proposals_panel_height(model))])
        .split(area);

    let cards = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(28), Constraint::Percentage(28), Constraint::Percentage(44)])
        .split(layout[0]);

    render_card(
        frame,
        cards[0],
        "Protocol",
        aligned_pair_lines(vec![
            ("Version", model.protocol_version.clone()),
            ("Epoch", model.tip.as_ref().map(|tip| format_count(tip.epoch)).unwrap_or_else(|| "—".into())),
            ("Slot", model.tip.as_ref().map(|tip| format_count(tip.slot)).unwrap_or_else(|| "—".into())),
        ]),
    );

    render_card(
        frame,
        cards[1],
        "Money Pots",
        aligned_pair_lines(vec![
            ("Treasury", model.treasury.map(format_lovelace).unwrap_or_else(|| "—".into())),
            ("Reserves", model.reserves.map(format_lovelace).unwrap_or_else(|| "—".into())),
            ("Fees", model.fees.map(format_lovelace).unwrap_or_else(|| "—".into())),
        ]),
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
    render_card(frame, cards[2], "Stake distribution", stake_lines);
    render_proposals_table(frame, layout[1], model, hotspots);
}

fn render_config(frame: &mut Frame<'_>, area: Rect, model: &Model) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(46), Constraint::Percentage(54)])
        .split(area);

    render_section_groups(frame, columns[0], &[&model.startup.runtime_sections, &model.startup.global_sections]);
    render_section_groups(frame, columns[1], &[&model.startup.protocol_sections]);
}

fn render_epoch_progress(frame: &mut Frame<'_>, area: Rect, model: &Model) {
    let Some(tip) = &model.tip else {
        return;
    };

    let epoch_length = model.startup.epoch_length.max(1);
    let slot_in_epoch = tip.slot_in_epoch.min(epoch_length);
    let ratio = slot_in_epoch as f64 / epoch_length as f64;
    let gauge = Gauge::default()
        .gauge_style(Style::default().fg(accent_primary()).bg(Color::Rgb(12, 22, 18)))
        .ratio(ratio)
        .label(format_ratio(slot_in_epoch, epoch_length))
        .block(
            Block::default()
                .title_top(
                    Line::from(vec![
                        Span::styled(" Epoch ", emphasis_primary()),
                        Span::styled(format_count(tip.epoch), emphasis_white()),
                        Span::styled(" ", emphasis_primary()),
                    ])
                    .left_aligned(),
                )
                .title_top(
                    Line::from(vec![
                        Span::styled(" ", emphasis_primary()),
                        Span::styled(format!("{:.1}%", ratio * 100.0), emphasis_white()),
                        Span::styled(" ", emphasis_primary()),
                    ])
                    .right_aligned(),
                )
                .borders(Borders::TOP)
                .border_style(border_primary()),
        );
    frame.render_widget(gauge, area);
}

fn render_logs(frame: &mut Frame<'_>, area: Rect, model: &Model, hotspots: &mut Hotspots) {
    hotspots.logs_area = area;
    let title = border_title_line(vec![
        Span::styled("Logs", emphasis_primary()),
        Span::styled(format!(" dropped={} ", model.dropped_logs), Style::default().fg(muted_color())),
    ]);
    let toggle_label = button_label(log_toggle_label(model));
    let toggle = border_title_line(vec![Span::styled(toggle_label.clone(), emphasis_primary())]);
    let block = Block::default()
        .title(title)
        .title_top(toggle.right_aligned())
        .borders(Borders::ALL)
        .border_style(border_primary());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    hotspots.log_toggle = Rect {
        x: area.x
            + area.width.saturating_sub(toggle_label.len() as u16 + border_title_chrome_width() + 1)
            + border_title_prefix_width(),
        y: area.y,
        width: toggle_label.len() as u16,
        height: 1,
    };

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(inner);

    render_log_controls(frame, layout[0], model, hotspots);

    let logs = model.filtered_logs();
    let visible = layout[1].height as usize;
    let scroll = model.log_scroll.min(logs.len().saturating_sub(visible));
    let end = logs.len().saturating_sub(scroll);
    let start = end.saturating_sub(visible);
    let lines = logs[start..end].iter().map(|record| log_record_line(record)).collect::<Vec<_>>();

    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, layout[1]);
    render_scrollbar(frame, layout[1], logs.len(), visible, start);
}

fn render_log_controls(frame: &mut Frame<'_>, area: Rect, model: &Model, hotspots: &mut Hotspots) {
    let layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(34), Constraint::Min(48)])
        .split(area);

    let level_spans = LevelFilter::ALL
        .into_iter()
        .map(|filter| {
            let style = if filter == model.level_filter {
                style_for_level_filter(filter).add_modifier(Modifier::BOLD)
            } else {
                muted()
            };
            hotspots.level_tabs.push((filter, Rect::default()));
            Span::styled(button_label(filter.label()), style)
        })
        .collect::<Vec<_>>();
    let levels = Paragraph::new(Line::from(level_spans))
        .alignment(Alignment::Left)
        .block(Block::default().borders(Borders::RIGHT).border_style(border_secondary()));
    frame.render_widget(levels, layout[0]);

    let target_spans = TargetFilter::ALL
        .into_iter()
        .map(|filter| {
            let style = if filter == model.target_filter { emphasis_primary() } else { muted() };
            hotspots.target_tabs.push((filter, Rect::default()));
            Span::styled(button_label(filter.label()), style)
        })
        .collect::<Vec<_>>();
    let targets = Paragraph::new(Line::from(target_spans)).alignment(Alignment::Right);
    frame.render_widget(targets, layout[1]);

    let level_labels = LevelFilter::ALL.into_iter().map(|filter| button_label(filter.label())).collect::<Vec<_>>();
    let mut level_x = layout[0].x;
    for ((_, rect), label) in hotspots.level_tabs.iter_mut().zip(level_labels.iter()) {
        *rect = Rect { x: level_x, y: layout[0].y, width: label.len() as u16, height: layout[0].height };
        level_x += label.len() as u16;
    }

    let target_labels = TargetFilter::ALL.into_iter().map(|filter| button_label(filter.label())).collect::<Vec<_>>();
    let mut target_x =
        layout[1].x + layout[1].width.saturating_sub(spans_width(target_labels.iter().map(|label| label.len() as u16)));
    for ((_, rect), label) in hotspots.target_tabs.iter_mut().zip(target_labels.iter()) {
        *rect = Rect { x: target_x, y: layout[1].y, width: label.len() as u16, height: layout[1].height };
        target_x += label.len() as u16;
    }
}

fn render_section_groups(frame: &mut Frame<'_>, area: Rect, groups: &[&[ConfigSection]]) {
    if groups.iter().all(|sections| sections.is_empty()) {
        return;
    }

    let sections = groups.iter().flat_map(|group| group.iter()).collect::<Vec<_>>();
    let constraints = sections
        .iter()
        .enumerate()
        .map(|(index, section)| {
            if index + 1 == sections.len() {
                Constraint::Min(section.entries.len() as u16 + 3)
            } else {
                Constraint::Length(section.entries.len() as u16 + 3)
            }
        })
        .collect::<Vec<_>>();
    let chunks = Layout::default().direction(Direction::Vertical).constraints(constraints).split(area);

    for (chunk, section) in chunks.iter().copied().zip(sections.iter()) {
        render_config_section(frame, chunk, section);
    }
}

fn render_config_section(frame: &mut Frame<'_>, area: Rect, section: &ConfigSection) {
    if section.entries.iter().all(|entry| entry.option.is_none() && entry.env_var.is_none()) {
        let rows = section
            .entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                Row::new(vec![
                    Cell::from(entry.label).style(Style::default().fg(Color::Rgb(215, 225, 235))),
                    Cell::from(entry.value.clone()).style(Style::default().fg(emphasis_white_color())),
                ])
                .style(striped_row_style(index))
            })
            .collect::<Vec<_>>();

        let table = Table::new(rows, [Constraint::Length(28), Constraint::Min(10)])
            .header(Row::new(vec!["Parameter", "Value"]).style(table_header_style()))
            .block(
                Block::default()
                    .title(block_title(section.title))
                    .borders(Borders::ALL)
                    .border_style(border_secondary()),
            );
        frame.render_widget(table, area);
        return;
    }

    let rows = section
        .entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            Row::new(vec![
                Cell::from(entry.option.unwrap_or("—")).style(Style::default().fg(Color::Rgb(215, 225, 235))),
                Cell::from(entry.env_var.unwrap_or("—")).style(Style::default().fg(Color::Rgb(170, 185, 205))),
                Cell::from(entry.value.clone()).style(Style::default().fg(emphasis_white_color())),
            ])
            .style(striped_row_style(index))
        })
        .collect::<Vec<_>>();

    let table = Table::new(rows, [Constraint::Length(28), Constraint::Length(34), Constraint::Min(10)])
        .header(Row::new(vec!["Option", "Env", "Value"]).style(table_header_style()))
        .block(
            Block::default().title(block_title(section.title)).borders(Borders::ALL).border_style(border_secondary()),
        );
    frame.render_widget(table, area);
}

fn render_peers_table(frame: &mut Frame<'_>, area: Rect, model: &Model, hotspots: &mut Hotspots) {
    hotspots.peers_area = area;
    let visible = area.height.saturating_sub(3) as usize;
    let start = model.peer_scroll.min(model.peers.len().saturating_sub(visible));
    let rows = model
        .peers
        .values()
        .skip(start)
        .take(visible)
        .enumerate()
        .map(|(index, peer)| peer_row(start + index, peer))
        .collect::<Vec<_>>();
    let block = Block::default().title(block_title("Peers")).borders(Borders::ALL).border_style(border_primary());
    let inner = block.inner(area);
    let table = Table::new(
        rows,
        [
            Constraint::Length(4),
            Constraint::Percentage(44),
            Constraint::Length(10),
            Constraint::Length(12),
            Constraint::Length(12),
            Constraint::Percentage(18),
        ],
    )
    .header(Row::new(vec!["Dir", "Peer", "State", "RTT", "Duplex", "Tags"]).style(table_header_style()))
    .block(block);
    frame.render_widget(table, area);
    render_scrollbar(frame, inner, model.peers.len(), visible, start);
}

fn render_proposals_table(frame: &mut Frame<'_>, area: Rect, model: &Model, hotspots: &mut Hotspots) {
    hotspots.proposals_area = area;
    let visible = area.height.saturating_sub(3) as usize;
    let start = model.proposal_scroll.min(model.recent_proposals.len().saturating_sub(visible));
    let rows = model
        .recent_proposals
        .iter()
        .skip(start)
        .take(visible)
        .enumerate()
        .map(|(index, proposal)| {
            Row::new(vec![
                Cell::from(proposal.id.clone()).style(Style::default().fg(emphasis_white_color())),
                Cell::from(proposal.kind.clone()).style(Style::default().fg(accent_primary())),
                Cell::from(proposal.detail.clone().unwrap_or_else(|| "—".into()))
                    .style(Style::default().fg(emphasis_white_color())),
                Cell::from(proposal.status.clone())
                    .style(Style::default().fg(accent_primary()).add_modifier(Modifier::BOLD)),
            ])
            .style(striped_row_style(start + index))
        })
        .collect::<Vec<_>>();
    let block = Block::default()
        .title(block_title(&format!(
            "Governance proposals ({})",
            model
                .governance
                .proposal_count_in_scope
                .map(format_count)
                .unwrap_or_else(|| format_count(model.recent_proposals.len()))
        )))
        .borders(Borders::ALL)
        .border_style(border_primary());
    let inner = block.inner(area);
    let table =
        Table::new(rows, [Constraint::Length(40), Constraint::Length(28), Constraint::Min(10), Constraint::Length(12)])
            .header(Row::new(vec!["id", "Kind", "Detail", "Status"]).style(table_header_style()))
            .block(block);
    frame.render_widget(table, area);
    render_scrollbar(frame, inner, model.recent_proposals.len(), visible, start);
}

fn render_series_card(frame: &mut Frame<'_>, area: Rect, title: &str, data: Vec<u64>, unit: &str) {
    let latest = data.last().copied().unwrap_or_default();
    let max = data.iter().copied().max().unwrap_or(1);
    let block = Block::default()
        .title(block_title(&format!("{title} · {} {unit}", format_count(latest))))
        .borders(Borders::ALL)
        .border_style(border_secondary());
    let sparkline = Sparkline::default().data(&data).style(Style::default().fg(accent_primary())).max(max).block(block);
    frame.render_widget(sparkline, area);
}

fn render_card(frame: &mut Frame<'_>, area: Rect, title: &str, lines: Vec<Line<'static>>) {
    let paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: true })
        .block(Block::default().title(block_title(title)).borders(Borders::ALL).border_style(border_secondary()));
    frame.render_widget(paragraph, area);
}

fn sample_memory_mib(model: &Model) -> Vec<u64> {
    model.system_samples.iter().map(|sample| sample.process_memory_bytes / 1_048_576).collect()
}

fn sample_cpu_tenths(model: &Model) -> Vec<u64> {
    model.system_samples.iter().map(|sample| sample.cpu_percent.round() as u64).collect()
}

fn sample_disk_kib(model: &Model) -> Vec<u64> {
    let mut previous: Option<&crate::events::SystemSample> = None;
    let mut rates = Vec::new();
    for sample in &model.system_samples {
        if let Some(prev) = previous {
            let read = sample.disk_read_bytes.saturating_sub(prev.disk_read_bytes);
            let write = sample.disk_write_bytes.saturating_sub(prev.disk_write_bytes);
            rates.push((read + write) / 1_024);
        } else {
            rates.push(0);
        }
        previous = Some(sample);
    }
    rates
}

fn peer_row(index: usize, peer: &PeerState) -> Row<'static> {
    let direction = match (peer.inbound, peer.outbound) {
        (true, true) => "↕",
        (true, false) => "↓",
        (false, true) => "↑",
        (false, false) => "·",
    };
    let state = if peer.connected { "online" } else { "offline" };
    let rtt =
        peer.last_rtt_micros.map(|value| format!("{:.1} ms", value as f64 / 1_000.0)).unwrap_or_else(|| "—".into());
    let duplex = match (peer.full_duplex, peer.full_duplex_capable) {
        (Some(true), _) => "full",
        (Some(false), Some(true)) => "half",
        (_, Some(false)) => "simplex",
        _ => "—",
    };
    let tags = if peer.trusted { "trusted" } else { "—" };

    Row::new(vec![
        Cell::from(direction).style(Style::default().fg(accent_primary())),
        Cell::from(peer.address.clone()).style(Style::default().fg(emphasis_white_color())),
        Cell::from(state).style(Style::default().fg(if peer.connected { accent_primary() } else { muted_color() })),
        Cell::from(rtt).style(Style::default().fg(emphasis_white_color())),
        Cell::from(duplex).style(Style::default().fg(muted_color())),
        Cell::from(tags).style(Style::default().fg(accent_primary())),
    ])
    .style(striped_row_style(index))
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

fn elide(value: &str, max: usize) -> String {
    if value.len() <= max {
        value.into()
    } else {
        format!("{}…{}", &value[..max / 2], &value[value.len().saturating_sub(max / 2 - 1)..])
    }
}

fn page_tabs_line(model: &Model) -> Line<'static> {
    let mut spans = Vec::new();

    for (index, page) in Page::ALL.into_iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw(" "));
        }
        let style = if page == model.page {
            emphasis_primary()
        } else {
            Style::default().fg(Color::Rgb(185, 198, 214)).add_modifier(Modifier::BOLD)
        };
        spans.push(Span::styled(button_label(page.label()), style));
    }

    border_title_line(spans)
}

fn shell_title(model: &Model) -> Line<'static> {
    border_title_line(vec![
        Span::styled("AMARU", emphasis_primary()),
        Span::raw("  "),
        Span::styled(model.startup.process.software_version.clone(), emphasis_white()),
        Span::raw("  "),
        Span::styled(format!("network={}", model.startup.process.network), emphasis_primary()),
    ])
}

fn shell_hint() -> Line<'static> {
    border_title_line(vec![
        Span::styled("<q>", emphasis_primary()),
        Span::styled("quit  ", muted()),
        Span::styled("<tab>", emphasis_primary()),
        Span::styled("next  ", muted()),
        Span::styled("<S-tab>", emphasis_primary()),
        Span::styled("prev  ", muted()),
        Span::styled("<mouse>", emphasis_primary()),
        Span::styled("navigate ", muted()),
    ])
}

fn window_controls_line(model: &Model) -> Line<'static> {
    let mut spans = Vec::new();

    for (index, window) in model.windows().iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw(" "));
        }
        let style = if index == model.selected_window {
            emphasis_primary()
        } else {
            Style::default().fg(Color::Rgb(210, 220, 235))
        };
        spans.push(Span::styled(button_label(&format_duration_short(*window)), style));
    }

    border_title_line(spans)
}

fn log_toggle_label(model: &Model) -> &'static str {
    if model.log_pane_mode.is_maximized() { "-" } else { "+" }
}

fn log_record_line(record: &crate::events::TelemetryRecord) -> Line<'static> {
    let fields = crate::model::render_fields(record);
    let mut spans = vec![
        Span::styled(format_log_wall_time(record.wall_time), muted()),
        Span::raw(" "),
        Span::styled(format!("{:>5}", record.level), style_for_level(record.level).add_modifier(Modifier::BOLD)),
        Span::raw(" "),
        Span::styled(record.target.clone(), style_for_target(&record.target).add_modifier(Modifier::BOLD)),
        Span::raw(" "),
        Span::styled(record.primary_label().to_string(), emphasis_white()),
    ];

    if record.kind == crate::events::TelemetryKind::SpanClose {
        spans.push(Span::styled(" close", Style::default().fg(accent_primary()).add_modifier(Modifier::BOLD)));
    }

    if !fields.is_empty() {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(fields, muted()));
    }

    Line::from(spans)
}

fn render_scrollbar(frame: &mut Frame<'_>, area: Rect, total: usize, visible: usize, position: usize) {
    if total <= visible || visible == 0 {
        return;
    }

    let mut state = ScrollbarState::new(total).position(position);
    state = state.viewport_content_length(visible);

    frame.render_stateful_widget(
        Scrollbar::new(ScrollbarOrientation::VerticalRight).begin_symbol(None).end_symbol(None),
        area,
        &mut state,
    );
}

fn button_label(label: &str) -> String {
    format!("[ {label} ]")
}

fn border_title_line(spans: Vec<Span<'static>>) -> Line<'static> {
    let mut line = Vec::with_capacity(spans.len() + 2);
    line.push(Span::styled("─ ", muted()));
    line.extend(spans);
    line.push(Span::styled(" ─", muted()));
    Line::from(line)
}

fn page_content_height(model: &Model) -> u16 {
    match model.page {
        Page::Amaru => 14 + peers_panel_height(model),
        Page::Cardano => 7 + proposals_panel_height(model),
        Page::Config => config_panel_height(model),
    }
}

fn peers_panel_height(model: &Model) -> u16 {
    panel_height(model.peers.len(), 4, 10)
}

fn proposals_panel_height(model: &Model) -> u16 {
    panel_height(model.recent_proposals.len(), 4, 10)
}

fn panel_height(rows: usize, min_rows: usize, max_rows: usize) -> u16 {
    rows.clamp(min_rows, max_rows).saturating_add(3) as u16
}

fn config_panel_height(model: &Model) -> u16 {
    config_column_height(&model.startup.runtime_sections)
        .saturating_add(config_column_height(&model.startup.global_sections))
        .max(config_column_height(&model.startup.protocol_sections))
}

fn config_column_height(sections: &[crate::startup::ConfigSection]) -> u16 {
    sections.iter().map(section_height).sum()
}

fn section_height(section: &crate::startup::ConfigSection) -> u16 {
    section.entries.len().saturating_add(3) as u16
}

fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);

    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

fn splash_logo_line(left_pad: usize) -> Line<'static> {
    Line::from(vec![
        Span::raw(" ".repeat(left_pad)),
        Span::styled("            ", Style::default().bg(accent_primary())),
    ])
}

fn border_title_prefix_width() -> u16 {
    2
}

fn border_title_chrome_width() -> u16 {
    4
}

fn spans_width(lengths: impl Iterator<Item = u16>) -> u16 {
    lengths.sum()
}

fn format_stake_distribution(active_stake: u64, max_lovelace_supply: u64) -> String {
    let percentage =
        if max_lovelace_supply == 0 { 0.0 } else { active_stake as f64 / max_lovelace_supply as f64 * 100.0 };
    format!("{} ({percentage:.1}%)", format_lovelace(active_stake))
}
