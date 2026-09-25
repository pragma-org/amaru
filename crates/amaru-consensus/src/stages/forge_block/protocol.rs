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

use std::{
    collections::{BTreeMap, BTreeSet},
    time::{Duration, SystemTime},
};

use amaru_kernel::{Epoch, EraHistory, HeaderHash, IsHeader, Nonce, ORIGIN_HASH, Point, Slot};
use amaru_observability::{error, info, warn};
use amaru_ouroboros::praos::nonce as praos_nonce;
use amaru_ouroboros_traits::{FindCommonAncestorResult, Nonces};
use amaru_protocols::store_effects::{Store, StoreBlockEffect, StoreValidatedHeaderEffect};
use amaru_pure_stage::{
    Effects, Instant, define_messages, define_role, define_role_tag, make_states, on_receive, typestate::prelude::*,
};

use super::{
    ForgeBlock, ForgeData, FreezeWatch,
    calc::{
        FORGE_LEAD_OFFSET, ForgeWindow, MissedSlotReason, ParentChoice, choose_parent, decide_freeze, forge_window,
        format_utc_timestamp, freeze_depth, instant_for_relative, lead_fire_at, missed_slot, ocert_covers,
        schedule_settled, wait_until_onset,
    },
    effects::{ForgeHeaderEffect, LeaderScheduleEffect, TakeForForgeEffect},
    schedule::{EpochSchedule, Schedule as Schedules},
};
use crate::{effects::ValidateHeaderEffect, stages::select_chain::SelectChainMsg};

make_states!(pub Live as LiveIn { Idle(IdleIn); Signed(!), Window(!) });

/// Witness for the second half of forging. Not a mailbox message: `DueLead`
/// finishes into [`Signed`] and receives this immediately, so the effect
/// sequence stays within the tuple limit.
struct Publish;

/// Witness that the forge window has been checked. Not a mailbox message.
struct Proceed;

define_role_tag!(pub ToSelectChain);
define_role!(pub SelectChainOut, ToSelectChain, SelectChainMsg);

define_messages! {
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub enum ForgeBlockMsg {
        AdoptedTip { tip: Point, parent: Point },
        DueLead { slot: Slot, generation: u64 },
        LeaderSchedule { schedule: EpochSchedule },
    }
}

/// Payload sent to `select_chain` after a block is stored.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ForgeTip {
    tip: Point,
    parent: Point,
}

impl From<ForgeTip> for SelectChainMsg {
    fn from(tip: ForgeTip) -> Self {
        SelectChainMsg::tip_from_upstream(tip.tip, tip.parent)
    }
}

on_receive!(Idle as IdleIn {
    AdoptedTip => {
        Clock,
        Repeat<Detach<LeaderScheduleEffect>>,
        Repeat<CancelSchedule>,
        Repeat<Schedule<DueLead>>
        => Idle
    }
    DueLead => {
        // Clock before the forge-or-miss choice: both alternatives would otherwise
        // start with Clock, and a shared choice head is ambiguous.
        Clock, Repeat<Wait> => Window
    }
    LeaderSchedule => {
        Clock, Repeat<CancelSchedule>, Repeat<Schedule<DueLead>> => Idle
    }
});

on_receive!(Window, Proceed =>
    External<TakeForForgeEffect>,
    External<ForgeHeaderEffect>,
    Repeat<Terminate> // KES signing failed
    => Signed
    | Repeat<CancelSchedule>, Repeat<Schedule<DueLead>> => Idle
);

on_receive!(Signed, Publish =>
    External<ValidateHeaderEffect>,
    External<StoreValidatedHeaderEffect>,
    External<StoreBlockEffect>,
    Clock,
    Repeat<Wait>,
    Send<ToSelectChain, ForgeTip>,
    Repeat<CancelSchedule>,
    Repeat<Schedule<DueLead>>
    => Idle
);

macro_rules! arm_next_lead {
    ($session:expr, $state:ident, $now:expr) => {{
        // Every scheduling decision, including "nothing to arm", invalidates a DueLead
        // that was already queued when its timer was cancelled.
        $state.schedule_generation = $state.schedule_generation.wrapping_add(1);
        drop_elapsed($state, $now);
        log_schedule($state, $now);
        if let Some((slot, when)) =
            next_lead_deadline(&$state.schedule, $state.consensus_parameters.era_history(), $now)
        {
            let generation = $state.schedule_generation;
            let (id, session) = $session.schedule_at(DueLead { slot, generation }, when).await;
            $state.next_lead = Some(id);
            session.finish()
        } else {
            $session.finish()
        }
    }};
}

macro_rules! finish_with_next_lead {
    ($session:expr, $state:ident, $now:expr) => {{
        if let Some(id) = $state.next_lead.take() {
            let (_cancelled, session) = $session.cancel_schedule(id).await;
            arm_next_lead!(session, $state, $now)
        } else {
            arm_next_lead!($session, $state, $now)
        }
    }};
}

/// Drive one mailbox message through the Idle remainder.
pub async fn stage(
    ForgeBlock { live, mut data }: ForgeBlock,
    msg: ForgeBlockMsg,
    eff: Effects<ForgeBlockMsg>,
) -> ForgeBlock {
    match live.convert_input(msg) {
        Ok(LiveIn::Idle(idle, IdleIn::AdoptedTip(tip))) => {
            let store = Store::new(eff.clone());
            let idle = handle_adopted_tip(&mut data, idle, tip, store, eff).await;
            ForgeBlock { live: idle.into(), data }
        }
        Ok(LiveIn::Idle(idle, IdleIn::DueLead(lead))) => {
            let idle = handle_due_lead(&mut data, idle, lead, eff).await;
            ForgeBlock { live: idle.into(), data }
        }
        Ok(LiveIn::Idle(idle, IdleIn::LeaderSchedule(schedule))) => {
            let idle = handle_leader_schedule(&mut data, idle, schedule, eff).await;
            ForgeBlock { live: idle.into(), data }
        }
        Err((live, _msg)) => ForgeBlock { live, data },
    }
}

async fn handle_adopted_tip(
    state: &mut ForgeData,
    idle: Idle,
    tip: AdoptedTip,
    store: Store,
    eff: Effects<ForgeBlockMsg>,
) -> Idle {
    state.adopted_tip = tip.tip;
    state.adopted_parent = tip.parent;
    let (now, session) = idle.receive(&tip, eff.clone()).clock().await;

    let mut to_schedule: BTreeSet<Epoch> = BTreeSet::new();
    if state.adopted_tip != Point::Origin
        && let Some(header) = store.load_header(&state.adopted_tip.hash()).await
    {
        let window = state.consensus_parameters.randomness_stabilization_window();
        if let Ok((epoch, in_stability_window)) =
            praos_nonce::randomness_stability_window(&header, state.consensus_parameters.era_history(), window)
        {
            // the “praos stability window” is the part from the beginning of the epoch until 3k/f slots before the end of the epoch
            let in_freeze_window = !in_stability_window;
            let tip_watch = FreezeWatch { epoch, point: header.point() };
            // Parent links of this tip, not the best-chain fragment: adopt_chain may
            // switch the chain again before this effect runs.
            let freeze_is_ancestor = match &state.freeze {
                Some(watch) => match store.find_common_ancestor(header.hash(), watch.point.hash()).await {
                    Ok(FindCommonAncestorResult::Found(point)) => point.hash() == watch.point.hash(),
                    Ok(_) => false,
                    Err(error) => {
                        error!(
                            consensus::block::INVARIANT_VIOLATED,
                            tip = state.adopted_tip,
                            invariant = error.to_string()
                        );
                        return eff.terminate().await;
                    }
                },
                None => true,
            };
            let decision =
                decide_freeze(state.freeze.as_ref(), tip_watch, in_freeze_window, freeze_is_ancestor, state.k);
            if let Some(drop) = decision.drop_epoch {
                state.schedule.forget(drop);
            }
            state.freeze = decision.freeze;
            // Slots below the tip's epoch can never be forged into: `choose_parent`
            // rejects any lead slot at or before the adopted tip.
            state.schedule.prune_before(epoch);
            if let Some(next) = decision.schedule_epoch {
                to_schedule.insert(next);
            }
            if !state.schedule.knows(epoch) {
                to_schedule.insert(epoch);
            }
        }
    }

    let mut session = session;
    for epoch in to_schedule {
        let Some(nonce) = schedule_nonce(state.adopted_tip, &store, epoch).await else {
            continue;
        };

        if !state.schedule.request(epoch, nonce) {
            continue;
        }

        let Some((from, until)) = epoch_slots(state.consensus_parameters.era_history(), epoch) else {
            state.schedule.forget(epoch);
            continue;
        };

        let effect = LeaderScheduleEffect::new(epoch, nonce, state.pool, from, until);
        session = session.detach(effect, |schedule| LeaderSchedule { schedule }.into()).await;
    }
    finish_with_next_lead!(session, state, now)
}

async fn handle_leader_schedule(
    state: &mut ForgeData,
    idle: Idle,
    msg: LeaderSchedule,
    eff: Effects<ForgeBlockMsg>,
) -> Idle {
    let (now, session) = idle.receive(&msg, eff).clock().await;

    // A result whose nonce no longer matches the outstanding request was computed
    // from a candidate nonce a rollback has since replaced; it is dropped.
    state.schedule.install(msg.schedule);

    finish_with_next_lead!(session, state, now)
}

async fn handle_due_lead(state: &mut ForgeData, idle: Idle, lead: DueLead, eff: Effects<ForgeBlockMsg>) -> Idle {
    let slot = lead.slot;
    let stale = lead.generation != state.schedule_generation;
    let cert = if stale { None } else { state.schedule.lead_at(slot).map(|scheduled| scheduled.cert().clone()) };
    if !stale {
        state.schedule.drop_through(slot);
    }

    let (mut now, mut session) = idle.receive(&lead, eff.clone()).clock().await;
    if stale {
        return session.finish().receive(&Proceed, eff).finish();
    }

    let woke_late = {
        let era_history = state.consensus_parameters.era_history();
        if let Some((onset, end)) = slot_bounds(era_history, now, slot) {
            if let ForgeWindow::Wait(delay) = forge_window(now, onset, end) {
                let (at, next) = session.wait(delay).await;
                session = next;
                now = at;
            }
            matches!(forge_window(now, onset, end), ForgeWindow::Late)
        } else {
            false
        }
    };
    if woke_late {
        warn!(consensus::forge::MISSED_SLOT, slot, reason = MissedSlotReason::WokeLate.as_str());
        let session = session.finish().receive(&Proceed, eff.clone());
        return finish_with_next_lead!(session, state, now);
    }

    let kes_period = state.consensus_parameters.slot_to_kes_period(slot);
    let coverage = ocert_covers(kes_period, state.ocert_start_period, state.consensus_parameters.max_kes_evolutions());
    let parent_choice = choose_parent(state.adopted_tip.slot(), slot);
    if let Some(reason) = missed_slot(coverage, parent_choice) {
        warn!(consensus::forge::MISSED_SLOT, slot, reason = reason.as_str());
        let session = session.finish().receive(&Proceed, eff.clone());
        return finish_with_next_lead!(session, state, now);
    }

    let Some(cert) = cert else {
        warn!(consensus::forge::MISSED_SLOT, slot, reason = MissedSlotReason::NotLed.as_str());
        let session = session.finish().receive(&Proceed, eff.clone());
        return finish_with_next_lead!(session, state, now);
    };

    let session = session.finish().receive(&Proceed, eff.clone());

    let parent_point = match parent_choice {
        ParentChoice::AdoptedTip => state.adopted_tip,
        ParentChoice::AdoptedParent => state.adopted_parent,
        ParentChoice::MissedTipAhead => unreachable!("filtered by missed_slot"),
    };
    let parent_hash: HeaderHash = parent_point.hash();
    let block_number = u64::from(parent_point.block_height()) + 1;

    let (body, session) = session.external(TakeForForgeEffect::new(parent_hash, slot)).await;
    let (header, session) =
        session.external(ForgeHeaderEffect::new(slot, parent_hash, block_number, &body, cert)).await;
    let header = match header {
        Ok(header) => header,
        Err(error) => {
            error!(consensus::forge::FORGE_FAILED, slot, step = "sign_header", error = error.to_string());
            return session.terminate().await;
        }
    };
    let session = session.finish().receive(&Publish, eff.clone());

    let (nonces, session) = session.external(ValidateHeaderEffect::new(&header)).await;
    let nonces: Nonces = match nonces {
        Ok(nonces) => nonces,
        Err(error) => {
            error!(consensus::forge::FORGE_FAILED, slot, step = "validate_header", error = error.to_string());
            // A header we just signed must validate; this is an invariant, not a protocol choice.
            return eff.terminate().await;
        }
    };

    let header_hash = header.hash();
    let header_point = header.point();
    let (stored, session) = session.external(StoreValidatedHeaderEffect::new(header, nonces)).await;
    if let Err(error) = stored {
        error!(consensus::forge::FORGE_FAILED, slot, step = "store_header", error = error.to_string());
        // Chain-store invariant; Amaru is shutting down regardless of this remainder.
        return eff.terminate().await;
    }

    let (stored, session) = session.external(StoreBlockEffect::new(&header_hash, body.block)).await;
    if let Err(error) = stored {
        error!(consensus::forge::FORGE_FAILED, slot, step = "store_block", error = error.to_string());
        return eff.terminate().await;
    }

    let (now, mut session) = session.clock().await;
    if let Some(onset) = slot_onset(state.consensus_parameters.era_history(), now, slot)
        && let Some(delay) = wait_until_onset(onset, now)
    {
        let (_at, next) = session.wait(delay).await;
        session = next;
    }
    let forged = ForgeTip { tip: header_point, parent: parent_point };
    info!(consensus::forge::FORGED, slot, header_hash, parent = parent_hash);
    let session = session.send(&state.select_chain, forged).await;
    finish_with_next_lead!(session, state, now)
}

fn log_schedule(state: &ForgeData, now: Instant) {
    let slots: BTreeMap<Epoch, usize> = state.schedule.led_counts();
    let next_slot = next_slot_timestamp(state, now);
    let any_led = slots.values().any(|&count| count > 0);
    if !any_led && next_slot.is_none() && state.freeze.is_none() {
        return;
    }
    let depth = state
        .freeze
        .as_ref()
        .map(|watch| freeze_depth(watch.point.block_height(), state.adopted_tip.block_height()))
        .unwrap_or(0);
    let settled = schedule_settled(depth, state.k);
    if let Some(next_slot) = next_slot {
        info!(consensus::forge::SCHEDULE, slots, next_slot, freeze_depth = depth, settled);
    } else {
        info!(consensus::forge::SCHEDULE, slots, freeze_depth = depth, settled);
    }
}

/// UTC onset of the next led slot that would be armed from `now`.
fn next_slot_timestamp(state: &ForgeData, now: Instant) -> Option<String> {
    let era_history = state.consensus_parameters.era_history();
    let (slot, _) = next_lead_deadline(&state.schedule, era_history, now)?;
    let relative = era_history.slot_to_relative_time_unchecked_horizon(slot).ok()?;
    let start = SystemTime::UNIX_EPOCH.checked_add(Duration::from_millis(state.system_start_unix_ms))?;
    format_utc_timestamp(start.checked_add(relative)?)
}

fn drop_elapsed(state: &mut ForgeData, now: Instant) {
    let era_history = state.consensus_parameters.era_history();
    let latest_closed =
        state.schedule.leads().map(|lead| lead.slot()).filter(|&slot| slot_closed(era_history, now, slot)).last();
    if let Some(slot) = latest_closed {
        state.schedule.drop_through(slot);
    }
}

fn next_lead_deadline(schedules: &Schedules, era_history: &EraHistory, now: Instant) -> Option<(Slot, Instant)> {
    let slot = schedules.leads().map(|lead| lead.slot()).find(|&slot| !slot_closed(era_history, now, slot))?;
    let onset = slot_onset(era_history, now, slot)?;
    Some((slot, lead_fire_at(onset, now, FORGE_LEAD_OFFSET)))
}

/// The forge window has closed: less than [`FORGE_LEAD_OFFSET`] remains before the slot ends.
/// A slot whose end cannot be placed is kept.
fn slot_closed(era_history: &EraHistory, now: Instant, slot: Slot) -> bool {
    slot_bounds(era_history, now, slot)
        .is_some_and(|(onset, end)| matches!(forge_window(now, onset, end), ForgeWindow::Late))
}

fn slot_bounds(era_history: &EraHistory, now: Instant, slot: Slot) -> Option<(Instant, Instant)> {
    Some((slot_onset(era_history, now, slot)?, slot_onset(era_history, now, slot + 1)?))
}

fn slot_onset(era_history: &EraHistory, now: Instant, slot: Slot) -> Option<Instant> {
    era_history.slot_to_relative_time_unchecked_horizon(slot).ok().map(|relative| instant_for_relative(now, relative))
}

fn epoch_slots(era_history: &EraHistory, epoch: Epoch) -> Option<(Slot, Slot)> {
    let from = era_history.epoch_bounds(epoch).ok()?.start;
    let until = era_history.epoch_bounds(epoch + 1).ok()?.start;
    Some((from, until))
}

/// Epoch nonce the leader schedule for `epoch` is computed from, as of `adopted_tip`.
async fn schedule_nonce(adopted_tip: Point, store: &Store, epoch: Epoch) -> Option<Nonce> {
    let header = store.load_header(&adopted_tip.hash()).await?;
    let nonces = store.get_nonces(&header.hash()).await?;
    if epoch == nonces.epoch {
        return Some(nonces.active);
    }
    let tail_parent = match store.load_header(&nonces.tail).await {
        Some(tail) => tail.parent().unwrap_or(ORIGIN_HASH),
        None => ORIGIN_HASH,
    };

    Some(nonces.next_active(tail_parent))
}
