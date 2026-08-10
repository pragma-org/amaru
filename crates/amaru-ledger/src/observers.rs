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

//! Optional hooks for embedders observing ledger progress.
//!
//! Observers are **outside** the consensus stage graph: the node never stops
//! itself based on these callbacks. The outer application decides lifecycle.
//!
//! # Semantics
//!
//! - Callbacks receive **short-lived references** and run on the ledger path.
//!   Long work or panics delay or abort ledger progress; clone needed pieces and
//!   hand them to another task if processing is non-trivial.
//! - UTxO deltas are a borrowed [`DiffSet`] of [`Arc<MemoizedTransactionOutput>`]
//!   so observers can [`Arc::clone`] only when they need ownership (no forced deep
//!   copy or refcount bump on the hot path).
//! - [`LedgerObservers::on_block`] (and the adopt/undo convenience setters) fire
//!   only after a successful commit of the corresponding tip change:
//!   - **Adopt**: after a roll-forward commit path (volatile push, stable apply
//!     if any, epoch transition flush).
//!   - **Undo**: after a successful [`crate::state::State::switch_to_fork`], for
//!     each discarded volatile block, **tip-first**, before the corresponding
//!     adopt events of the new branch. Failed fork switches emit nothing.
//! - [`LedgerObservers::on_ledger_snapshot`] is opt-in and only runs when installed.
//!   [`StakeSummary`] / [`crate::summary::stake_distribution::StakeDistribution`]
//!   do not implement [`Clone`]; their fields (`accounts`, `pools`, `dreps`,
//!   scalars) and piece types ([`crate::summary::AccountState`],
//!   [`crate::summary::PoolState`], [`crate::summary::governance::DRepState`])
//!   do — clone only what you need.

use std::sync::Arc;

use amaru_kernel::{Block, Epoch, MemoizedTransactionOutput, Point, Tip, TransactionInput, TransactionRef};

use crate::{
    state::volatile::{AnchoredVolatileFragment, DiffSet, VolatileFragment},
    summary::stake_distribution::StakeSummary,
};

/// UTxO delta carried by a tip block event (produced + consumed).
pub type UtxoDiff = DiffSet<TransactionInput, Arc<MemoizedTransactionOutput>>;

/// A block that was successfully applied to the ledger tip.
///
/// The source [`Block`] and UTxO [`DiffSet`] are borrowed for the duration of the
/// observer callback. Clone individual fields (or [`Arc::clone`] outputs) only if
/// you need them after the callback returns.
#[derive(Debug, Clone, Copy)]
pub struct AdoptedBlock<'a> {
    pub point: Point,
    pub epoch: Epoch,
    pub block_height: u64,
    /// Source block; transaction bodies / witnesses / aux data live here.
    pub block: &'a Block,
    /// UTxO produced/consumed by this block.
    pub utxo: &'a UtxoDiff,
}

impl<'a> AdoptedBlock<'a> {
    pub fn from_block(point: Point, epoch: Epoch, block: &'a Block, fragment: &'a VolatileFragment) -> Self {
        let block_height = block.header.header_body.block_number;
        Self { point, epoch, block_height, block, utxo: &fragment.utxo }
    }

    /// Transactions in block order (bodies, witnesses, validity, aux data) — all borrowed.
    pub fn transactions(&self) -> impl Iterator<Item = TransactionRef<'a>> + '_ {
        let invalid = self.block.invalid_transactions.as_ref();
        self.block.transaction_bodies.iter().zip(self.block.transaction_witnesses.iter()).enumerate().map(
            move |(ix, (body, witnesses))| TransactionRef {
                body,
                witnesses: witnesses.as_ref(),
                is_expected_valid: invalid.is_none_or(|set| !set.contains(&(ix as u16))),
                auxiliary_data: self.block.auxiliary_data.get(&(ix as u16)),
            },
        )
    }
}

/// A previously adopted volatile block that was discarded (fork switch / rollback).
///
/// Transaction bodies are not retained on the volatile fragment, so undos expose the
/// UTxO delta only. Embedders that index by address should remove `utxo.produced` and
/// re-admit `utxo.consumed` keys (the pre-spend outputs must already be known from prior
/// adopt events or a bootstrap index).
#[derive(Debug, Clone, Copy)]
pub struct UndoneBlock<'a> {
    pub point: Point,
    pub epoch: Epoch,
    pub block_height: u64,
    /// UTxO produced/consumed by the undone block.
    pub utxo: &'a UtxoDiff,
}

impl<'a> UndoneBlock<'a> {
    pub fn from_anchored(fragment: &'a AnchoredVolatileFragment, epoch: Epoch) -> Self {
        let tip: Tip = fragment.tip();
        let point = tip.point();
        let block_height = u64::from(tip.block_height());
        Self { point, epoch, block_height, utxo: &fragment.fragment.utxo }
    }
}

/// Tip-relative block lifecycle delivered to [`LedgerObservers::on_block`].
///
/// Borrowed so the ledger does not force an extra clone of payload data.
#[derive(Debug, Clone, Copy)]
pub enum LedgerBlockEvent<'a> {
    /// A block was successfully applied to the tip.
    Adopted(AdoptedBlock<'a>),
    /// A previously applied volatile block was discarded (tip-first on fork switch).
    Undone(UndoneBlock<'a>),
}

/// Full stake summary that the node would otherwise discard after deriving the slim
/// in-memory [`crate::summary::stake_distribution::StakeDistribution`].
///
/// Opt-in only. Callbacks receive a short-lived reference while the ledger still
/// holds the value. The type is not [`Clone`]; clone individual fields / map
/// entries (`AccountState`, `PoolState`, `DRepState`, …) when ownership is needed.
pub type LedgerStateSnapshot = StakeSummary;

/// Callbacks installed on ledger [`crate::state::State`].
///
/// Handlers take references so the ledger does not force a clone. Callers that
/// need to process data off the ledger thread should clone only the fields they
/// need and hand them to another task.
#[derive(Clone, Default)]
pub struct LedgerObservers {
    /// Unified tip block lifecycle (adopt + undo). Prefer this for reorg-safe indexes.
    pub on_block: Option<Arc<dyn Fn(LedgerBlockEvent<'_>) + Send + Sync>>,
    /// Invoked when a full stake summary is computed, before only the slim distribution is kept.
    pub on_ledger_snapshot: Option<Arc<dyn Fn(&LedgerStateSnapshot) + Send + Sync>>,
}

impl LedgerObservers {
    pub fn new() -> Self {
        Self::default()
    }

    /// Install a unified block lifecycle handler (adopt and undo).
    pub fn on_block(mut self, handler: impl Fn(LedgerBlockEvent<'_>) + Send + Sync + 'static) -> Self {
        self.on_block = Some(Arc::new(handler));
        self
    }

    /// Convenience: handle only successful adopts (ignores undos).
    ///
    /// Prefer [`Self::on_block`] when maintaining external UTxO indexes across fork switches.
    pub fn on_adopted_block(mut self, handler: impl Fn(AdoptedBlock<'_>) + Send + Sync + 'static) -> Self {
        self.on_block = Some(Arc::new(move |event| {
            if let LedgerBlockEvent::Adopted(block) = event {
                handler(block);
            }
        }));
        self
    }

    /// Convenience: handle only undos (ignores adopts).
    pub fn on_undone_block(mut self, handler: impl Fn(UndoneBlock<'_>) + Send + Sync + 'static) -> Self {
        self.on_block = Some(Arc::new(move |event| {
            if let LedgerBlockEvent::Undone(block) = event {
                handler(block);
            }
        }));
        self
    }

    pub fn on_ledger_snapshot(mut self, handler: impl Fn(&LedgerStateSnapshot) + Send + Sync + 'static) -> Self {
        self.on_ledger_snapshot = Some(Arc::new(handler));
        self
    }

    pub(crate) fn notify_adopted(&self, adopted: AdoptedBlock<'_>) {
        if let Some(cb) = &self.on_block {
            cb(LedgerBlockEvent::Adopted(adopted));
        }
    }

    pub(crate) fn notify_undone(&self, undone: UndoneBlock<'_>) {
        if let Some(cb) = &self.on_block {
            cb(LedgerBlockEvent::Undone(undone));
        }
    }

    pub(crate) fn wants_block_events(&self) -> bool {
        self.on_block.is_some()
    }
}
