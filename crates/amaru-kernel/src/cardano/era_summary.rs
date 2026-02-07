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

use crate::{Epoch, EraBound, Slot, cardano::era_params::EraParams, cbor};
use std::time::Duration;

// The start is inclusive and the end is exclusive. In a valid EraHistory, the
// end of each era will equal the start of the next one.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EraSummary {
    pub start: EraBound,
    pub end: Option<EraBound>,
    pub params: EraParams,
}

impl EraSummary {
    /// Checks whether the current `EraSummary` ends after the given slot; In case
    /// where the EraSummary doesn't have any upper bound, then we check whether the
    /// point is within a foreseeable horizon.
    pub fn contains_slot(&self, slot: &Slot, tip: &Slot, stability_window: &Slot) -> bool {
        &self
            .end
            .as_ref()
            .map(|end| end.slot)
            .unwrap_or_else(|| self.calculate_end_bound(tip, stability_window).slot)
            >= slot
    }

    /// Like contains_slot, but doesn't enforce anything about the upper bound. So when there's no
    /// upper bound, the slot is simply always considered within the era.
    pub fn contains_slot_unchecked_horizon(&self, slot: &Slot) -> bool {
        self.end
            .as_ref()
            .map(|end| &end.slot >= slot)
            .unwrap_or(true)
    }

    pub fn contains_epoch(&self, epoch: &Epoch, tip: &Slot, stability_window: &Slot) -> bool {
        &self
            .end
            .as_ref()
            .map(|end| end.epoch)
            .unwrap_or_else(|| self.calculate_end_bound(tip, stability_window).epoch)
            > epoch
    }

    /// Like contains_epoch, but doesn't enforce anything about the upper bound. So when there's no
    /// upper bound, the epoch is simply always considered within the era.
    pub fn contains_epoch_unchecked_horizon(&self, epoch: &Epoch) -> bool {
        self.end
            .as_ref()
            .map(|end| &end.epoch > epoch)
            .unwrap_or(true)
    }

    /// Calculate a virtual end `EraBound` given a time and the last era summary that we know of.
    ///
    /// **pre-condition**: the provided tip must be after (or equal) to the start of this era.
    fn calculate_end_bound(&self, tip: &Slot, stability_window: &Slot) -> EraBound {
        let Self { start, params, end } = self;

        debug_assert!(end.is_none());

        // NOTE: The +1 here is justified by the fact that upper bound in era summaries are
        // exclusive. So if our tip is *exactly* at the frontier of the stability area, then
        // technically, we already can foresee time in the next epoch.
        let end_of_stable_window =
            start.slot.as_u64().max(tip.as_u64() + 1) + stability_window.as_u64();

        let delta_slots = end_of_stable_window - start.slot.as_u64();

        let delta_epochs = delta_slots / params.epoch_size_slots
            + if delta_slots.is_multiple_of(params.epoch_size_slots) {
                0
            } else {
                1
            };

        let max_foreseeable_epoch = start.epoch.as_u64() + delta_epochs;

        let foreseeable_slots = delta_epochs * params.epoch_size_slots;

        EraBound {
            time: Duration::from_secs(
                start.time.as_secs() + params.slot_length.as_secs() * foreseeable_slots,
            ),
            slot: Slot::new(start.slot.as_u64() + foreseeable_slots),
            epoch: Epoch::new(max_foreseeable_epoch),
        }
    }
}

impl<C> cbor::Encode<C> for EraSummary {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.begin_array()?;
        self.start.encode(e, ctx)?;
        self.end.encode(e, ctx)?;
        self.params.encode(e, ctx)?;
        e.end()?;
        Ok(())
    }
}

impl<'b, C> cbor::Decode<'b, C> for EraSummary {
    fn decode(d: &mut cbor::Decoder<'b>, _ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        cbor::heterogeneous_array(d, |d, assert_len| {
            assert_len(3)?;
            let start = d.decode()?;
            let end = d.decode()?;
            let params = d.decode()?;
            Ok(EraSummary { start, end, params })
        })
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    use super::*;
    use crate::{Epoch, any_era_bound_for_epoch, any_era_params, prop_cbor_roundtrip};
    use proptest::prelude::*;
    use std::cmp::{max, min};

    prop_compose! {
        pub fn any_era_summary()(
            b1 in any::<u16>(),
            b2 in any::<u16>(),
            params in any_era_params(),
        )(
            first_epoch in Just(min(b1, b2) as u64),
            last_epoch in Just(max(b1, b2) as u64),
            params in Just(params),
            start in any_era_bound_for_epoch(Epoch::from(max(b1, b2) as u64)),
        ) -> EraSummary {
            let epochs_elapsed = last_epoch - first_epoch;
            let slots_elapsed = epochs_elapsed * params.epoch_size_slots;
            let time_elapsed = params.slot_length * slots_elapsed as u32;
            let end = Some(EraBound {
                time: start.time + time_elapsed,
                slot: start.slot.offset_by(slots_elapsed),
                epoch: Epoch::from(last_epoch),
            });
            EraSummary { start, end, params }
        }
    }

    prop_cbor_roundtrip!(EraSummary, any_era_summary());
}
