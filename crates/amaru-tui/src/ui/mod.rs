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
    widgets::{Block, BorderType, Borders, Cell, Clear, Paragraph, Row, Sparkline, Table, Wrap},
};

use self::{
    format::{
        aligned_pair_lines, format_count, format_density, format_duration, format_log_wall_time, format_lovelace,
        format_ratio, format_slot_ratio,
    },
    theme::{
        accent_primary, block_title, border_primary, border_secondary, emphasis_primary, emphasis_white,
        emphasis_white_color, muted, muted_color, striped_row_style, style_for_level, style_for_level_filter,
        style_for_target, table_header_style,
    },
};
use crate::{
    config::TimeWindow,
    model::{InteractionMode, LevelFilter, Model, Page, PeerState, ScrollFocus, TargetFilter},
    startup::ConfigSection,
};

mod format;
mod theme;
mod views;

pub use self::views::Views;

pub fn render(frame: &mut Frame<'_>, model: &Model, views: &mut Views, now: Instant) {
    views.reset();

    let is_ready = model.is_ready(now);
    let progress_height = u16::from(model.tip.is_some()) * 2;
    let shell = shell_block(model, is_ready);
    let shell_area = frame.area();
    let inner = shell.inner(shell_area);

    frame.render_widget(shell, shell_area);
    if !is_ready {
        render_splash(frame, inner, model);
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

    let available_height = inner.height.saturating_sub(progress_height);
    let show_logs = model.page != Page::Config;

    if show_logs {
        let max_content_height = available_height.saturating_sub(10);
        let desired_content_height = if is_ready { page_content_height(model) } else { max_content_height };
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

        if is_ready {
            match model.page {
                Page::Amaru => render_amaru(frame, layout[0], model, views, now),
                Page::Cardano => render_cardano(frame, layout[0], model, views, now),
                Page::Config => render_config(frame, layout[0], model),
            }
        } else {
            render_splash(frame, layout[0], model);
        }

        render_logs(frame, layout[1], model, views);

        if progress_height > 0 {
            render_epoch_progress(frame, layout[2], model);
        }
    } else {
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(available_height), Constraint::Length(progress_height)])
            .split(inner);

        if is_ready {
            render_config(frame, layout[0], model);
        } else {
            render_splash(frame, layout[0], model);
        }

        if progress_height > 0 {
            render_epoch_progress(frame, layout[1], model);
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
            .title_bottom(shell_hint(model).left_aligned())
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

fn render_splash(frame: &mut Frame<'_>, area: Rect, model: &Model) {
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
    let paragraph = Paragraph::new(logo.lines).alignment(Alignment::Center).wrap(Wrap { trim: false });

    frame.render_widget(Clear, area);
    frame.render_widget(paragraph, logo_popup);
    render_splash_progress(frame, layout[3], model, &progress_states);
}

fn render_amaru(frame: &mut Frame<'_>, area: Rect, model: &Model, views: &mut Views, now: Instant) {
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(7),
            Constraint::Length(7),
            Constraint::Min(peers_panel_height(model).max(health_panel_height()).max(mempool_panel_height())),
        ])
        .split(area);

    let cards = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(22),
            Constraint::Percentage(34),
            Constraint::Percentage(18),
            Constraint::Percentage(26),
        ])
        .split(layout[0]);

    render_card(
        frame,
        cards[0],
        "Node",
        aligned_pair_lines(vec![
            ("Version", model.startup.process.software_version.clone()),
            ("Target", model.startup.process.target.clone()),
            ("Protocol", model.protocol_version.clone()),
            ("Uptime", format_duration(now.duration_since(model.created_at))),
        ]),
        model.interaction_mode,
    );

    let tip_lines = if let Some(tip) = &model.tip {
        aligned_pair_lines(vec![
            ("Epoch", format_count(tip.epoch)),
            ("Slot", format_slot_ratio(tip.slot, model.startup.target_slot())),
            ("Height", format_count(tip.block_height)),
            ("Hash", tip.header_hash.clone()),
        ])
    } else {
        vec![Line::from(Span::styled("No tip telemetry yet", muted()))]
    };
    render_card(frame, cards[1], "Tip", tip_lines, model.interaction_mode);

    let block_rate = blocks_per_second(model, now);
    let transaction_rate = transactions_per_second(model, now);
    render_card(
        frame,
        cards[2],
        "Throughput",
        aligned_pair_lines(vec![
            ("Blocks", format_count(model.blocks_in_window(now))),
            ("Txs", format_count(model.transactions_in_window(now))),
            ("Blocks/s", format!("{block_rate:.2}")),
            ("Tx/s", format!("{transaction_rate:.2}")),
        ]),
        model.interaction_mode,
    );

    render_card(
        frame,
        cards[3],
        "Chain quality",
        aligned_pair_lines(vec![
            (
                "Last block",
                model
                    .last_block_elapsed(now)
                    .map(|duration| format!("{} ago", format_duration(duration)))
                    .unwrap_or_else(|| "—".into()),
            ),
            (
                "Chain Density",
                model
                    .tip
                    .as_ref()
                    .map(|tip| format_density(tip.density, model.startup.active_slot_coeff_inverse))
                    .unwrap_or_else(|| "—".into()),
            ),
            (
                "Avg rollback",
                model
                    .average_rollback_length(now)
                    .map(|value| format!("{value:.1} blocks"))
                    .unwrap_or_else(|| "—".into()),
            ),
        ]),
        model.interaction_mode,
    );

    let charts = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(34), Constraint::Percentage(33), Constraint::Percentage(33)])
        .split(layout[1]);
    render_series_card(frame, charts[0], "Memory", sample_memory_mib(model), "MiB", model.interaction_mode);
    render_series_card(frame, charts[1], "CPU", sample_cpu_tenths(model), "%", model.interaction_mode);
    render_series_card(frame, charts[2], "Disk I/O", sample_disk_kib(model), "KiB/s", model.interaction_mode);

    let bottom = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(54), Constraint::Percentage(23), Constraint::Percentage(23)])
        .split(layout[2]);
    render_peers_table(frame, bottom[0], model, views);
    render_card(
        frame,
        bottom[1],
        "Network health",
        aligned_pair_lines(vec![
            ("RTT", model.average_rtt_millis().map(|value| format!("{value:.1} ms")).unwrap_or_else(|| "—".into())),
            ("Inbound", format_count(model.inbound_peer_count())),
            ("Outbound", format_count(model.outbound_peer_count())),
        ]),
        model.interaction_mode,
    );
    render_card(
        frame,
        bottom[2],
        "Mempool",
        aligned_pair_lines(vec![
            ("Txs", format_count(model.mempool.tx_count)),
            ("Occupancy", format_kib_ratio(model.mempool.size_bytes, model.startup.mempool_max_bytes)),
        ]),
        model.interaction_mode,
    );
}

fn render_cardano(frame: &mut Frame<'_>, area: Rect, model: &Model, views: &mut Views, _now: Instant) {
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
        .constraints([
            Constraint::Percentage(22),
            Constraint::Percentage(22),
            Constraint::Percentage(20),
            Constraint::Percentage(36),
        ])
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

fn render_config(frame: &mut Frame<'_>, area: Rect, model: &Model) {
    let left_width = if show_config_env_column(area) { 60 } else { 64 };
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(left_width), Constraint::Percentage(100 - left_width)])
        .split(area);

    render_section_groups(
        frame,
        columns[0],
        &[&model.startup.runtime_sections, &model.startup.global_sections],
        model.interaction_mode,
    );
    render_section_groups(frame, columns[1], &[&model.startup.protocol_sections], model.interaction_mode);
}

fn render_epoch_progress(frame: &mut Frame<'_>, area: Rect, model: &Model) {
    let Some(tip) = &model.tip else {
        return;
    };

    let epoch_length = model.startup.epoch_length.max(1);
    let slot_in_epoch = tip.slot_in_epoch.min(epoch_length);
    let ratio = slot_in_epoch as f64 / epoch_length as f64;
    let block = Block::default()
        .title_top(
            border_title_line(
                vec![
                    Span::styled("Epoch", emphasis_primary(model.interaction_mode)),
                    Span::raw(" "),
                    Span::styled(format_count(tip.epoch), emphasis_white()),
                ],
                model.interaction_mode,
                false,
            )
            .left_aligned(),
        )
        .title_top(
            border_title_line(
                vec![Span::styled(format!("{:.1}%", ratio * 100.0), emphasis_white())],
                model.interaction_mode,
                false,
            )
            .right_aligned(),
        )
        .borders(Borders::TOP)
        .border_style(border_primary(model.interaction_mode));
    let inner = block.inner(area);

    frame.render_widget(block, area);
    render_gradient_progress_bar(frame, inner, ratio, &format_ratio(slot_in_epoch, epoch_length));
}

fn render_logs(frame: &mut Frame<'_>, area: Rect, model: &Model, views: &mut Views) {
    views.logs_area = area;
    let focused = model.scroll_focus == ScrollFocus::Logs;
    let title = border_title_line(
        vec![Span::styled("Logs", emphasis_primary(model.interaction_mode))],
        model.interaction_mode,
        focused,
    );
    let toggle_label = button_label(log_toggle_label(model));
    let toggle = border_title_line(
        vec![Span::styled(toggle_label.clone(), emphasis_primary(model.interaction_mode))],
        model.interaction_mode,
        focused,
    );
    let block = Block::default()
        .title(title)
        .title_top(toggle.right_aligned())
        .borders(Borders::ALL)
        .border_style(scroll_panel_border(focused, model.interaction_mode))
        .border_type(scroll_panel_border_type(focused));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    views.log_toggle = Rect {
        x: area.x
            + area.width.saturating_sub(toggle_label.len() as u16 + border_title_chrome_width() + 1)
            + border_title_prefix_width(),
        y: area.y,
        width: toggle_label.len() as u16,
        height: 1,
    };

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1), Constraint::Min(1)])
        .split(inner);

    render_log_controls(frame, layout[0], model, views);
    render_horizontal_separator(frame, layout[1], model.interaction_mode, focused);

    let logs = model.filtered_logs();
    let visible = layout[2].height as usize;
    let scroll = model.log_scroll.min(logs.len().saturating_sub(visible));
    let end = logs.len().saturating_sub(scroll);
    let start = end.saturating_sub(visible);
    let lines =
        logs[start..end].iter().map(|record| log_record_line(record, model.interaction_mode)).collect::<Vec<_>>();

    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, layout[2]);
    render_scrollbar(frame, layout[2], logs.len(), visible, start, model.interaction_mode);
}

fn render_log_controls(frame: &mut Frame<'_>, area: Rect, model: &Model, views: &mut Views) {
    let level_width = level_controls_width();
    let layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(level_width), Constraint::Length(1), Constraint::Min(48)])
        .split(area);

    let level_spans = LevelFilter::ALL
        .into_iter()
        .map(|filter| {
            let style = if filter == model.level_filter {
                style_for_level_filter(filter).add_modifier(Modifier::BOLD)
            } else {
                muted()
            };
            views.level_tabs.push((filter, Rect::default()));
            Span::styled(button_label(filter.label()), style)
        })
        .collect::<Vec<_>>();
    let levels = Paragraph::new(Line::from(level_spans)).alignment(Alignment::Left);
    frame.render_widget(levels, layout[0]);

    let target_spans = TargetFilter::ALL
        .into_iter()
        .map(|filter| {
            let style = if filter == model.target_filter { emphasis_primary(model.interaction_mode) } else { muted() };
            views.target_tabs.push((filter, Rect::default()));
            Span::styled(button_label(filter.label()), style)
        })
        .collect::<Vec<_>>();
    let targets = Paragraph::new(Line::from(target_spans)).alignment(Alignment::Right);
    frame.render_widget(targets, layout[2]);

    let level_labels = LevelFilter::ALL.into_iter().map(|filter| button_label(filter.label())).collect::<Vec<_>>();
    let mut level_x = layout[0].x;
    for ((_, rect), label) in views.level_tabs.iter_mut().zip(level_labels.iter()) {
        *rect = Rect { x: level_x, y: layout[0].y, width: label.len() as u16, height: layout[0].height };
        level_x += label.len() as u16;
    }

    let target_labels = TargetFilter::ALL.into_iter().map(|filter| button_label(filter.label())).collect::<Vec<_>>();
    let mut target_x =
        layout[2].x + layout[2].width.saturating_sub(spans_width(target_labels.iter().map(|label| label.len() as u16)));
    for ((_, rect), label) in views.target_tabs.iter_mut().zip(target_labels.iter()) {
        *rect = Rect { x: target_x, y: layout[2].y, width: label.len() as u16, height: layout[2].height };
        target_x += label.len() as u16;
    }
}

fn render_section_groups(frame: &mut Frame<'_>, area: Rect, groups: &[&[ConfigSection]], mode: InteractionMode) {
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
        render_config_section(frame, chunk, section, mode);
    }
}

fn render_config_section(frame: &mut Frame<'_>, area: Rect, section: &ConfigSection, mode: InteractionMode) {
    if section.entries.iter().all(|entry| entry.option.is_none() && entry.env_var.is_none()) {
        let rows = section
            .entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                Row::new(vec![
                    Cell::from(entry.label.as_str()).style(Style::default().fg(Color::Rgb(215, 225, 235))),
                    Cell::from(entry.value.clone()).style(Style::default().fg(emphasis_white_color())),
                ])
                .style(striped_row_style(index))
            })
            .collect::<Vec<_>>();

        let table = Table::new(rows, [Constraint::Length(28), Constraint::Min(10)])
            .header(Row::new(vec!["Parameter", "Value"]).style(table_header_style(mode)))
            .block(
                Block::default()
                    .title(block_title(mode, &section.title))
                    .borders(Borders::ALL)
                    .border_style(border_secondary(mode)),
            );
        frame.render_widget(table, area);
        return;
    }

    let table = if show_config_env_column(area) {
        let rows = section
            .entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                Row::new(vec![
                    Cell::from(entry.option.as_deref().unwrap_or("—"))
                        .style(Style::default().fg(Color::Rgb(215, 225, 235))),
                    Cell::from(entry.env_var.as_deref().unwrap_or("—"))
                        .style(Style::default().fg(Color::Rgb(170, 185, 205))),
                    Cell::from(entry.value.clone()).style(Style::default().fg(emphasis_white_color())),
                ])
                .style(striped_row_style(index))
            })
            .collect::<Vec<_>>();

        Table::new(rows, [Constraint::Length(28), Constraint::Length(34), Constraint::Min(10)])
            .header(Row::new(vec!["Option", "Env", "Value"]).style(table_header_style(mode)))
            .block(
                Block::default()
                    .title(block_title(mode, &section.title))
                    .borders(Borders::ALL)
                    .border_style(border_secondary(mode)),
            )
    } else {
        let rows = section
            .entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                Row::new(vec![
                    Cell::from(entry.option.as_deref().unwrap_or("—"))
                        .style(Style::default().fg(Color::Rgb(215, 225, 235))),
                    Cell::from(entry.value.clone()).style(Style::default().fg(emphasis_white_color())),
                ])
                .style(striped_row_style(index))
            })
            .collect::<Vec<_>>();

        Table::new(rows, [Constraint::Length(28), Constraint::Min(10)])
            .header(Row::new(vec!["Option", "Value"]).style(table_header_style(mode)))
            .block(
                Block::default()
                    .title(block_title(mode, &section.title))
                    .borders(Borders::ALL)
                    .border_style(border_secondary(mode)),
            )
    };

    frame.render_widget(table, area);
}

fn render_peers_table(frame: &mut Frame<'_>, area: Rect, model: &Model, views: &mut Views) {
    views.peers_area = area;
    let focused = model.scroll_focus == ScrollFocus::Peers;
    let toggle_label = button_label(peer_toggle_label(model));
    let block = Block::default()
        .title(panel_title(model.interaction_mode, focused, &format!("Peers ({})", format_count(model.peers.len()))))
        .title_top(
            border_title_line(
                vec![Span::styled(toggle_label.clone(), emphasis_primary(model.interaction_mode))],
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
    let start = model.peer_scroll.min(model.peers.len().saturating_sub(visible));
    let rows = model
        .peers
        .values()
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
            Constraint::Length(4),
            Constraint::Min(24),
            Constraint::Length(10),
            Constraint::Length(11),
            Constraint::Length(5),
        ],
    )
    .header(Row::new(vec!["", "Peer", "RTT", "Can duplex?", "Dir"]).style(table_header_style(model.interaction_mode)))
    .column_spacing(1)
    .block(block);
    frame.render_widget(table, area);
    render_scrollbar(frame, body, model.peers.len(), visible, start, model.interaction_mode);
}

fn render_proposals_table(frame: &mut Frame<'_>, area: Rect, model: &Model, views: &mut Views) {
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
                vec![Span::styled(toggle_label.clone(), emphasis_primary(model.interaction_mode))],
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

fn render_series_card(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &str,
    data: Vec<u64>,
    unit: &str,
    mode: InteractionMode,
) {
    let latest = data.last().copied().unwrap_or_default();
    let max = data.iter().copied().max().unwrap_or(1);
    let block = Block::default()
        .title(block_title(mode, &format!("{title} · {} {unit}", format_count(latest))))
        .borders(Borders::ALL)
        .border_style(border_secondary(mode));
    let sparkline =
        Sparkline::default().data(&data).style(Style::default().fg(accent_primary(mode))).max(max).block(block);
    frame.render_widget(sparkline, area);
}

fn render_card(frame: &mut Frame<'_>, area: Rect, title: &str, lines: Vec<Line<'static>>, mode: InteractionMode) {
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: true }).block(
        Block::default().title(block_title(mode, title)).borders(Borders::ALL).border_style(border_secondary(mode)),
    );
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
        Cell::from(peer.address.clone()).style(Style::default().fg(emphasis_white_color())),
        Cell::from(rtt).style(Style::default().fg(emphasis_white_color())),
        Cell::from(can_duplex).style(Style::default().fg(muted_color())),
        Cell::from(direction).style(Style::default().fg(accent_primary(mode))),
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
            Span::raw("  "),
            Span::styled(
                format!("network={}", model.startup.process.network),
                emphasis_primary(model.interaction_mode),
            ),
        ],
        model.interaction_mode,
        false,
    )
}

fn shell_hint(model: &Model) -> Line<'static> {
    if model.is_copy_mode() {
        return border_title_line(
            vec![Span::styled("<esc>", emphasis_primary(model.interaction_mode)), Span::styled("return", muted())],
            model.interaction_mode,
            false,
        );
    }

    border_title_line(
        vec![
            Span::styled("<q>", emphasis_primary(model.interaction_mode)),
            Span::styled("quit  ", muted()),
            Span::styled("<tab>", emphasis_primary(model.interaction_mode)),
            Span::styled("next  ", muted()),
            Span::styled("<S-tab>", emphasis_primary(model.interaction_mode)),
            Span::styled("prev  ", muted()),
            Span::styled("<f>", emphasis_primary(model.interaction_mode)),
            Span::styled("focus  ", muted()),
            Span::styled("<↑↓>", emphasis_primary(model.interaction_mode)),
            Span::styled("scroll  ", muted()),
            Span::styled("<mouse>", emphasis_primary(model.interaction_mode)),
            Span::styled("navigate ", muted()),
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

fn log_toggle_label(model: &Model) -> &'static str {
    if model.log_pane_mode.is_maximized() { "-" } else { "+" }
}

fn proposal_toggle_label(model: &Model) -> &'static str {
    if model.proposal_pane_mode.is_maximized() { "-" } else { "+" }
}

fn peer_toggle_label(model: &Model) -> &'static str {
    if model.peer_pane_mode.is_maximized() { "-" } else { "+" }
}

fn log_record_line(record: &crate::events::TelemetryRecord, mode: InteractionMode) -> Line<'static> {
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
        spans.push(Span::styled(" close", Style::default().fg(accent_primary(mode)).add_modifier(Modifier::BOLD)));
    }

    if !fields.is_empty() {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(fields, muted()));
    }

    Line::from(spans)
}

fn render_scrollbar(
    frame: &mut Frame<'_>,
    area: Rect,
    total: usize,
    visible: usize,
    position: usize,
    mode: InteractionMode,
) {
    if total <= visible || visible == 0 {
        return;
    }

    let height = area.height as usize;
    if height == 0 || area.width == 0 {
        return;
    }

    let x = area.x + area.width.saturating_sub(1);
    let track_style = border_secondary(mode);
    let thumb_style = Style::default().fg(accent_primary(mode)).add_modifier(Modifier::BOLD);
    let thumb_height = height.min(2);
    let max_position = total.saturating_sub(visible);
    let max_offset = height.saturating_sub(thumb_height);
    let top_offset = if max_position == 0 { 0 } else { position.saturating_mul(max_offset) / max_position.max(1) };
    let buffer = frame.buffer_mut();

    for offset in 0..height {
        let symbol = if (top_offset..top_offset + thumb_height).contains(&offset) { "█" } else { "│" };
        let style = if symbol == "█" { thumb_style } else { track_style };
        buffer.set_string(x, area.y + offset as u16, symbol, style);
    }
}

fn button_label(label: &str) -> String {
    format!("[ {label} ]")
}

fn window_label(window: &TimeWindow) -> String {
    button_label(&window.to_string())
}

fn border_title_line(spans: Vec<Span<'static>>, mode: InteractionMode, focused: bool) -> Line<'static> {
    let style = scroll_panel_border(focused, mode);
    let horizontal = if focused { "═" } else { "─" };
    let mut line = Vec::with_capacity(spans.len() + 2);
    line.push(Span::styled(format!("{horizontal} "), style));
    line.extend(spans);
    line.push(Span::styled(format!(" {horizontal}"), style));
    Line::from(line)
}

fn page_content_height(model: &Model) -> u16 {
    match model.page {
        Page::Amaru => 14 + peers_panel_height(model).max(health_panel_height()).max(mempool_panel_height()),
        Page::Cardano => 7 + proposals_panel_height(model),
        Page::Config => config_panel_height(model),
    }
}

fn peers_panel_height(model: &Model) -> u16 {
    panel_height(model.peers.len(), 4, 10)
}

fn proposals_panel_height(model: &Model) -> u16 {
    panel_height(model.proposal_order.len(), 4, 10)
}

fn panel_height(rows: usize, min_rows: usize, max_rows: usize) -> u16 {
    rows.clamp(min_rows, max_rows).saturating_add(3) as u16
}

fn health_panel_height() -> u16 {
    6
}

fn mempool_panel_height() -> u16 {
    6
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

fn border_title_prefix_width() -> u16 {
    2
}

fn border_title_chrome_width() -> u16 {
    4
}

fn show_config_env_column(area: Rect) -> bool {
    area.width >= 140
}

fn spans_width(lengths: impl Iterator<Item = u16>) -> u16 {
    lengths.sum()
}

fn format_stake_distribution(active_stake: u64, max_lovelace_supply: u64) -> String {
    let percentage =
        if max_lovelace_supply == 0 { 0.0 } else { active_stake as f64 / max_lovelace_supply as f64 * 100.0 };
    format!("{} ({percentage:.1}%)", format_lovelace(active_stake))
}

fn format_kib(bytes: u64) -> String {
    format!("{} KiB", format_count(bytes.div_ceil(1_024)))
}

fn format_kib_ratio(bytes: u64, capacity_bytes: u64) -> String {
    format!("{} / {}", format_kib(bytes), format_kib(capacity_bytes))
}

fn level_controls_width() -> u16 {
    spans_width(LevelFilter::ALL.into_iter().map(|filter| button_label(filter.label()).len() as u16))
}

fn table_body_area(inner: Rect) -> Rect {
    Rect { x: inner.x, y: inner.y.saturating_add(1), width: inner.width, height: inner.height.saturating_sub(1) }
}

fn scroll_panel_border(focused: bool, mode: InteractionMode) -> Style {
    if focused { emphasis_primary(mode) } else { border_primary(mode) }
}

fn scroll_panel_border_type(focused: bool) -> BorderType {
    if focused { BorderType::Double } else { BorderType::Plain }
}

fn panel_title(mode: InteractionMode, focused: bool, title: &str) -> Line<'static> {
    let style = scroll_panel_border(focused, mode);
    let horizontal = if focused { "═" } else { "─" };
    Line::from(vec![
        Span::styled(format!("{horizontal} "), style),
        Span::styled(title.to_string(), emphasis_primary(mode)),
        Span::styled(format!(" {horizontal}"), style),
    ])
}

fn render_horizontal_separator(frame: &mut Frame<'_>, area: Rect, mode: InteractionMode, focused: bool) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let horizontal = if focused { "═" } else { "─" }.repeat(area.width as usize);
    frame.render_widget(Paragraph::new(horizontal).style(border_secondary(mode)), area);
}

fn format_vote_status(status: Option<bool>) -> &'static str {
    match status {
        Some(true) => "yes",
        Some(false) => "no",
        None => "-",
    }
}

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
    progress_states: &[&crate::model::InitialStakeDistributionState],
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
                format!("Epoch {} · {:.0}%", format_count(state.epoch), state.progress * 100.0),
            )
        } else {
            (0.0, "Pending...".to_string())
        };

        render_gradient_progress_bar(frame, Rect { x: inner.x, y, width: gauge_width, height: 1 }, ratio, &label);
        y += 2;
    }
}

const LOGO_SHAPES: &[&[(f32, f32)]] = &[
    &[(814.326, 745.646), (822.949, 654.382), (833.843, 661.216), (834.603, 759.386)],
    &[(776.692, 764.223), (803.084, 623.251), (816.716, 633.17), (802.478, 783.86)],
    &[(689.882, 747.099), (768.258, 580.04), (799.003, 600.275), (760.908, 803.755)],
];

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

fn render_gradient_progress_bar(frame: &mut Frame<'_>, area: Rect, ratio: f64, label: &str) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let width = area.width as usize;
    let filled = ((ratio.clamp(0.0, 1.0) * area.width as f64).round() as usize).min(width);
    let label_chars = label.chars().collect::<Vec<_>>();
    let label_len = label_chars.len().min(width);
    let label_start = width.saturating_sub(label_len) / 2;
    let label_end = label_start + label_len;

    let spans = (0..width)
        .map(|index| {
            let is_filled = index < filled;
            let background = if is_filled {
                amaru_gradient_color(if width <= 1 { 0.0 } else { index as f32 / (width - 1) as f32 })
            } else {
                Color::Rgb(12, 22, 18)
            };

            if (label_start..label_end).contains(&index) {
                let glyph = label_chars[index - label_start];
                let foreground = if is_filled { Color::Rgb(8, 17, 14) } else { emphasis_white_color() };
                Span::styled(
                    glyph.to_string(),
                    Style::default().fg(foreground).bg(background).add_modifier(Modifier::BOLD),
                )
            } else {
                Span::styled(" ", Style::default().bg(background))
            }
        })
        .collect::<Vec<_>>();

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn amaru_gradient_color(position: f32) -> Color {
    interpolate_gradient(
        [(0.0, (48_u8, 228_u8, 161_u8)), (0.55, (48_u8, 207_u8, 178_u8)), (1.0, (49_u8, 130_u8, 243_u8))],
        position,
    )
}

fn interpolate_gradient(stops: [(f32, (u8, u8, u8)); 3], position: f32) -> Color {
    let clamped = position.clamp(0.0, 1.0);

    for [(start_offset, start), (end_offset, end)] in stops.array_windows() {
        if clamped <= *end_offset {
            let span = (end_offset - start_offset).max(f32::EPSILON);
            let t = ((clamped - start_offset) / span).clamp(0.0, 1.0);
            return Color::Rgb(
                interpolate_channel(start.0, end.0, t),
                interpolate_channel(start.1, end.1, t),
                interpolate_channel(start.2, end.2, t),
            );
        }
    }

    let (_, (r, g, b)) = stops[stops.len() - 1];
    Color::Rgb(r, g, b)
}

fn interpolate_channel(start: u8, end: u8, t: f32) -> u8 {
    (start as f32 + ((end as f32 - start as f32) * t)).round() as u8
}
