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
    mem,
    time::{Duration, Instant},
};

use amaru_kernel::{EraHistory, Point};
use amaru_metrics::ledger::LedgerMetrics;
use amaru_observability::debug;

pub const DEBOUNCE: Duration = Duration::from_millis(800);

#[derive(Debug, Default)]
pub struct TipUpdateEmitter {
    last_emitted_at: Option<Instant>,
    pending_tx_count: u64,
}

impl TipUpdateEmitter {
    pub fn notify(&mut self, now: Instant, point: &Point, metrics: &LedgerMetrics, era_history: &EraHistory) {
        let Some(tx_count) = self.observe(now, metrics.tx_count) else {
            return;
        };

        let slot = point.slot_or_default();
        let epoch = era_history
            .slot_to_epoch(slot, slot)
            .unwrap_or_else(|e| unreachable!("impossible; failed to compute epoch from current slot ({slot}): {e}"));
        let slot_in_epoch = era_history.slot_in_epoch(slot, slot).unwrap_or_else(|e| {
            unreachable!("impossible; failed to compute relative slot from current slot ({slot}): {e}")
        });

        debug!(
            amaru_observability::amaru::ledger::tip::UPDATE,
            slot,
            header_hash = point.hash(),
            block_height = metrics.block_height,
            tx_count = tx_count as usize,
            epoch,
            slot_in_epoch,
            density = metrics.density,
            current_kes_period = metrics.current_kes_period,
            remaining_kes_periods = metrics.remaining_kes_periods,
        );
    }

    fn observe(&mut self, now: Instant, tx_count: u64) -> Option<u64> {
        self.pending_tx_count = self.pending_tx_count.saturating_add(tx_count);

        let should_emit =
            self.last_emitted_at.is_none_or(|last_emitted_at| now.duration_since(last_emitted_at) >= DEBOUNCE);

        if !should_emit {
            return None;
        }

        self.last_emitted_at = Some(now);
        Some(mem::take(&mut self.pending_tx_count))
    }
}

#[cfg(test)]
mod tests {
    use super::{DEBOUNCE, TipUpdateEmitter};

    #[test]
    fn tip_update_emitter_batches_until_debounce_elapses() {
        let mut emitter = TipUpdateEmitter::default();
        let start = std::time::Instant::now();

        assert_eq!(emitter.observe(start, 3), Some(3));
        assert_eq!(emitter.observe(start + DEBOUNCE / 2, 5), None);
        assert_eq!(emitter.observe(start + DEBOUNCE, 7), Some(12));
    }
}
