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

use std::collections::BTreeSet;

use amaru_kernel::{ConsensusParameters, Epoch, EraHistory, HeaderHash, IsHeader, ORIGIN_HASH, Point, PoolId, Slot};
use amaru_observability::{error, info, warn};
use amaru_ouroboros::praos::nonce as praos_nonce;
use amaru_ouroboros_traits::{FindCommonAncestorResult, Nonces};
use amaru_protocols::store_effects::{Store, StoreBlockEffect, StoreValidatedHeaderEffect};
use amaru_pure_stage::{
    Effects, Instant, define_messages, define_role, define_role_tag, make_states, on_receive, typestate::prelude::*,
};

use super::{
    ForgeBlock, FreezeWatch,
    calc::{
        FORGE_LEAD_OFFSET, ParentChoice, choose_parent, decide_freeze, drop_past_led_slots, freeze_depth,
        instant_for_relative, lead_fire_at, missed_slot, next_schedulable_slot, ocert_covers, replace_epoch_slots,
        schedule_settled, wait_until_onset,
    },
    effects::{ForgeHeaderEffect, LeaderScheduleEffect, TakeForForgeEffect},
};
use crate::{effects::ValidateHeaderEffect, stages::select_chain::SelectChainMsg};

make_states!(pub Live { Idle });

define_role_tag!(pub ToSelectChain);
define_role!(pub SelectChainOut, ToSelectChain, SelectChainMsg);

define_messages! {
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub enum ForgeBlockMsg {
        AdoptedTip { tip: Point, parent: Point },
        LeadSlot { slot: Slot, generation: u64 },
        LeaderSchedule { epoch: Epoch, slots: Vec<Slot> },
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
        Repeat<Schedule<LeadSlot>>
        => Idle
    }
    LeadSlot => {
        Clock, Repeat<CancelSchedule>, Repeat<Schedule<LeadSlot>> => Idle
        | External<TakeForForgeEffect>,
          External<ForgeHeaderEffect>,
          External<ValidateHeaderEffect>,
          External<StoreValidatedHeaderEffect>,
          External<StoreBlockEffect>,
          Clock,
          Repeat<Wait>,
          Send<ToSelectChain, ForgeTip>,
          Repeat<CancelSchedule>,
          Repeat<Schedule<LeadSlot>>
          => Idle
    }
    LeaderSchedule => {
        Clock, Repeat<CancelSchedule>, Repeat<Schedule<LeadSlot>> => Idle
    }
});

macro_rules! arm_next_lead {
    ($session:expr, $state:ident, $now:expr) => {{
        // Every scheduling decision, including "nothing to arm", invalidates a LeadSlot
        // that was already queued when its timer was cancelled.
        $state.schedule_generation = $state.schedule_generation.wrapping_add(1);
        if let Some((slot, when)) =
            next_lead_deadline(&$state.led_slots, $state.consensus_parameters.era_history(), $now)
        {
            let generation = $state.schedule_generation;
            let (id, session) = $session.schedule_at(LeadSlot { slot, generation }, when).await;
            $state.next_lead = Some(id);
            $state.live = session.finish().into();
        } else {
            $state.live = $session.finish().into();
        }
    }};
}

macro_rules! finish_with_next_lead {
    ($session:expr, $state:ident, $now:expr) => {{
        if let Some(id) = $state.next_lead.take() {
            let (_cancelled, session) = $session.cancel_schedule(id).await;
            arm_next_lead!(session, $state, $now);
        } else {
            arm_next_lead!($session, $state, $now);
        }
    }};
}

/// Drive one mailbox message through the Idle remainder.
pub async fn stage(state: ForgeBlock, msg: ForgeBlockMsg, eff: Effects<ForgeBlockMsg>) -> ForgeBlock {
    let input = match &state.live {
        Live::Idle(idle) => idle.convert_input(msg),
    };
    match input {
        Ok(IdleIn::AdoptedTip(tip)) => {
            let store = Store::new(eff.clone());
            handle_adopted_tip(state, tip, store, eff).await
        }
        Ok(IdleIn::LeadSlot(lead)) => handle_lead_slot(state, lead, eff).await,
        Ok(IdleIn::LeaderSchedule(schedule)) => handle_leader_schedule(state, schedule, eff).await,
        Err(_msg) => state,
    }
}

async fn handle_adopted_tip(
    mut state: ForgeBlock,
    tip: AdoptedTip,
    store: Store,
    eff: Effects<ForgeBlockMsg>,
) -> ForgeBlock {
    // Move `live` out so returning `state` without `Session::finish` is a compile error.
    let Live::Idle(idle) = state.live;
    state.adopted_tip = tip.tip;
    state.adopted_parent = tip.parent;
    let (now, session) = idle.receive(&tip, eff.clone()).clock().await;

    let mut to_schedule: BTreeSet<Epoch> = BTreeSet::new();
    let mut tip_epoch = None;
    if state.adopted_tip != Point::Origin
        && let Some(header) = store.load_header(&state.adopted_tip.hash()).await
    {
        let window = state.consensus_parameters.randomness_stabilization_window();
        if let Ok((epoch, in_stability_window)) =
            praos_nonce::randomness_stability_window(&header, state.consensus_parameters.era_history(), window)
        {
            // the “praos stability window” is the part from the beginning of the epoch until 3k/f slots before the end of the epoch
            let in_freeze_window = !in_stability_window;
            tip_epoch = Some(epoch);
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
                state.pending_epochs.remove(&drop);
                if state.predicted_epoch == Some(drop) {
                    state.predicted_epoch = None;
                }
                if let Ok(bounds) = state.consensus_parameters.era_history().epoch_bounds(drop) {
                    replace_epoch_slots(&mut state.led_slots, bounds.start, epoch_until(bounds.end), []);
                }
            }
            if state.predicted_epoch == Some(epoch) {
                state.predicted_epoch = None;
            }
            state.freeze = decision.freeze;
            if let Some(watch) = state.freeze.as_ref() {
                let depth = freeze_depth(watch.point.block_height(), header.block_height());
                info!(
                    consensus::forge::SCHEDULE,
                    epoch = watch.scheduled_epoch(),
                    n_slots = state.led_slots.len(),
                    freeze_depth = depth,
                    settled = schedule_settled(depth, state.k)
                );
            }
            if let Some(next) = decision.schedule_epoch {
                to_schedule.insert(next);
            }
            if state.led_slots.is_empty() && state.pending_epochs.is_empty() {
                to_schedule.insert(epoch);
            }
        }
    }

    let mut session = session;
    for epoch in to_schedule {
        if !state.pending_epochs.insert(epoch) {
            continue;
        }
        if let Some(effect) =
            leader_schedule_effect(&state.consensus_parameters, state.adopted_tip, state.pool, &store, epoch).await
        {
            if tip_epoch.is_some_and(|current| epoch == current + 1) {
                state.predicted_epoch = Some(epoch);
            }
            session = session.detach(effect, move |slots| LeaderSchedule { epoch, slots }.into()).await;
        } else {
            state.pending_epochs.remove(&epoch);
        }
    }
    finish_with_next_lead!(session, state, now);
    state
}

async fn handle_leader_schedule(
    mut state: ForgeBlock,
    schedule: LeaderSchedule,
    eff: Effects<ForgeBlockMsg>,
) -> ForgeBlock {
    let Live::Idle(idle) = state.live;
    state.pending_epochs.remove(&schedule.epoch);
    let (now, session) = idle.receive(&schedule, eff).clock().await;
    if let Ok(bounds) = state.consensus_parameters.era_history().epoch_bounds(schedule.epoch) {
        let until = epoch_until(bounds.end);
        replace_epoch_slots(&mut state.led_slots, bounds.start, until, schedule.slots.iter().copied());
        let era_history = state.consensus_parameters.era_history();
        drop_past_led_slots(&mut state.led_slots, now, |slot| slot_onset(era_history, now, slot).unwrap_or(now));
        let depth = state
            .freeze
            .as_ref()
            .map(|watch| freeze_depth(watch.point.block_height(), state.adopted_tip.block_height()))
            .unwrap_or(0);
        info!(
            consensus::forge::SCHEDULE,
            epoch = schedule.epoch,
            n_slots = state.led_slots.len(),
            freeze_depth = depth,
            settled = schedule_settled(depth, state.k)
        );
    }

    finish_with_next_lead!(session, state, now);
    state
}

async fn handle_lead_slot(mut state: ForgeBlock, lead: LeadSlot, eff: Effects<ForgeBlockMsg>) -> ForgeBlock {
    let Live::Idle(idle) = state.live;
    let slot = lead.slot;
    if lead.generation != state.schedule_generation {
        let (_now, session) = idle.receive(&lead, eff).clock().await;
        state.live = session.finish().into();
        return state;
    }
    state.led_slots.retain(|&s| s > slot);
    let kes_period = state.consensus_parameters.slot_to_kes_period(slot);
    let coverage = ocert_covers(kes_period, state.ocert_start_period, state.consensus_parameters.max_kes_evolutions());
    let parent_choice = choose_parent(state.adopted_tip.slot(), slot);
    if let Some(reason) = missed_slot(coverage, parent_choice) {
        warn!(consensus::forge::MISSED_SLOT, slot, reason = reason.as_str());
        let (now, session) = idle.receive(&lead, eff).clock().await;
        finish_with_next_lead!(session, state, now);
        return state;
    }

    let parent_point = match parent_choice {
        ParentChoice::AdoptedTip => state.adopted_tip,
        ParentChoice::AdoptedParent => state.adopted_parent,
        ParentChoice::MissedTipAhead => unreachable!("filtered by missed_slot"),
    };
    let parent_hash: HeaderHash = parent_point.hash();
    let block_number = u64::from(parent_point.block_height()) + 1;

    let session = idle.receive(&lead, eff.clone());
    let (body, session) = session.external(TakeForForgeEffect::new(parent_hash, slot)).await;
    let (header, session) = session.external(ForgeHeaderEffect::new(slot, parent_hash, block_number, &body)).await;
    let header = match header {
        Ok(header) => header,
        Err(error) => {
            error!(consensus::forge::FORGE_FAILED, slot, step = "sign_header", error = error.to_string());
            // Not in the remainder: a sequence is at most 10 effects, and signing
            // failure shuts the node down the same way a store error does.
            return eff.terminate().await;
        }
    };

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
    finish_with_next_lead!(session, state, now);
    state
}

fn next_lead_deadline(led_slots: &[Slot], era_history: &EraHistory, now: Instant) -> Option<(Slot, Instant)> {
    let (slot, onset) = next_schedulable_slot(led_slots, now, |slot| slot_onset(era_history, now, slot))?;
    Some((slot, lead_fire_at(onset, now, FORGE_LEAD_OFFSET)))
}

fn epoch_until(end: Option<Slot>) -> Slot {
    end.unwrap_or(Slot::from(u64::MAX))
}

fn slot_onset(era_history: &EraHistory, now: Instant, slot: Slot) -> Option<Instant> {
    era_history.slot_to_relative_time_unchecked_horizon(slot).ok().map(|relative| instant_for_relative(now, relative))
}

async fn leader_schedule_effect(
    consensus_parameters: &ConsensusParameters,
    adopted_tip: Point,
    pool: PoolId,
    store: &Store,
    epoch: Epoch,
) -> Option<LeaderScheduleEffect> {
    let bounds = consensus_parameters.era_history().epoch_bounds(epoch).ok()?;
    let until = epoch_until(bounds.end);
    let header = store.load_header(&adopted_tip.hash()).await?;
    let nonces = store.get_nonces(&header.hash()).await?;
    let nonce = if epoch == nonces.epoch {
        nonces.active
    } else {
        let tail_parent = match store.load_header(&nonces.tail).await {
            Some(tail) => tail.parent().unwrap_or(ORIGIN_HASH),
            None => ORIGIN_HASH,
        };
        nonces.next_active(tail_parent)
    };
    Some(LeaderScheduleEffect::new(epoch, nonce, pool, bounds.start, until))
}
