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

use crate::{
    Epoch, EraName, MAINNET_GLOBAL_PARAMETERS, PREPROD_GLOBAL_PARAMETERS, Slot,
    TESTNET_GLOBAL_PARAMETERS, TimeMs,
    cardano::{era_params::EraParams, slot::SlotArithmeticError},
    cbor,
};
use std::{fs::File, io::BufReader, path::Path, sync::LazyLock};

/// Era history for Mainnet retrieved with:
///
/// ```bash
/// curl -X POST "https://mainnet.koios.rest/api/v1/ogmios"
///  -H 'accept: application/json'
///  -H 'content-type: application/json'
///  -d '{"jsonrpc":"2.0","method":"queryLedgerState/eraSummaries"}' | jq -c '.result'
/// ```
///
pub static MAINNET_ERA_HISTORY: LazyLock<EraHistory> = LazyLock::new(|| {
    let eras: [Summary; 7] = [
        Summary {
            start: Bound {
                time_ms: TimeMs::from(0),
                slot: Slot::from(0),
                epoch: Epoch::from(0),
            },
            end: Some(Bound {
                time_ms: TimeMs::from(89856000000),
                slot: Slot::from(4492800),
                epoch: Epoch::from(208),
            }),
            params: EraParams {
                epoch_size_slots: 21600,
                slot_length: TimeMs::new(20000),
                era_name: EraName::Byron,
            },
        },
        Summary {
            start: Bound {
                time_ms: TimeMs::from(89856000000),
                slot: Slot::from(4492800),
                epoch: Epoch::from(208),
            },
            end: Some(Bound {
                time_ms: TimeMs::from(101952000000),
                slot: Slot::from(16588800),
                epoch: Epoch::from(236),
            }),
            params: EraParams {
                epoch_size_slots: 432000,
                slot_length: TimeMs::new(1000),
                era_name: EraName::Shelley,
            },
        },
        Summary {
            start: Bound {
                time_ms: TimeMs::from(101952000000),
                slot: Slot::from(16588800),
                epoch: Epoch::from(236),
            },
            end: Some(Bound {
                time_ms: TimeMs::from(108432000000),
                slot: Slot::from(23068800),
                epoch: Epoch::from(251),
            }),
            params: EraParams {
                epoch_size_slots: 432000,
                slot_length: TimeMs::new(1000),
                era_name: EraName::Allegra,
            },
        },
        Summary {
            start: Bound {
                time_ms: TimeMs::from(108432000000),
                slot: Slot::from(23068800),
                epoch: Epoch::from(251),
            },
            end: Some(Bound {
                time_ms: TimeMs::from(125280000000),
                slot: Slot::from(39916800),
                epoch: Epoch::from(290),
            }),
            params: EraParams {
                epoch_size_slots: 432000,
                slot_length: TimeMs::new(1000),
                era_name: EraName::Mary,
            },
        },
        Summary {
            start: Bound {
                time_ms: TimeMs::from(125280000000),
                slot: Slot::from(39916800),
                epoch: Epoch::from(290),
            },
            end: Some(Bound {
                time_ms: TimeMs::from(157680000000),
                slot: Slot::from(72316800),
                epoch: Epoch::from(365),
            }),
            params: EraParams {
                epoch_size_slots: 432000,
                slot_length: TimeMs::new(1000),
                era_name: EraName::Alonzo,
            },
        },
        Summary {
            start: Bound {
                time_ms: TimeMs::from(157680000000),
                slot: Slot::from(72316800),
                epoch: Epoch::from(365),
            },
            end: Some(Bound {
                time_ms: TimeMs::from(219024000000),
                slot: Slot::from(133660800),
                epoch: Epoch::from(507),
            }),
            params: EraParams {
                epoch_size_slots: 432000,
                slot_length: TimeMs::new(1000),
                era_name: EraName::Babbage,
            },
        },
        Summary {
            start: Bound {
                time_ms: TimeMs::from(219024000000),
                slot: Slot::from(133660800),
                epoch: Epoch::from(507),
            },
            end: None,
            params: EraParams {
                epoch_size_slots: 432000,
                slot_length: TimeMs::new(1000),
                era_name: EraName::Conway,
            },
        },
    ];
    EraHistory::new(&eras, MAINNET_GLOBAL_PARAMETERS.stability_window)
});

/// Era history for Preprod retrieved with:
///
/// ```bash
/// curl -X POST "https://preprod.koios.rest/api/v1/ogmios"
///  -H 'accept: application/json'
///  -H 'content-type: application/json'
///  -d '{"jsonrpc":"2.0","method":"queryLedgerState/eraSummaries"}' | jq -c '.result'
/// ```
///
pub static PREPROD_ERA_HISTORY: LazyLock<EraHistory> = LazyLock::new(|| {
    let eras: [Summary; 7] = [
        Summary {
            start: Bound {
                time_ms: TimeMs::from(0),
                slot: Slot::from(0),
                epoch: Epoch::from(0),
            },
            end: Some(Bound {
                time_ms: TimeMs::from(1728000000),
                slot: Slot::from(86400),
                epoch: Epoch::from(4),
            }),
            params: EraParams {
                epoch_size_slots: 21600,
                slot_length: TimeMs::new(20000),
                era_name: EraName::Byron,
            },
        },
        Summary {
            start: Bound {
                time_ms: TimeMs::from(1728000000),
                slot: Slot::from(86400),
                epoch: Epoch::from(4),
            },
            end: Some(Bound {
                time_ms: TimeMs::from(2160000000),
                slot: Slot::from(518400),
                epoch: Epoch::from(5),
            }),
            params: EraParams {
                epoch_size_slots: 432000,
                slot_length: TimeMs::new(1000),
                era_name: EraName::Shelley,
            },
        },
        Summary {
            start: Bound {
                time_ms: TimeMs::from(2160000000),
                slot: Slot::from(518400),
                epoch: Epoch::from(5),
            },
            end: Some(Bound {
                time_ms: TimeMs::from(2592000000),
                slot: Slot::from(950400),
                epoch: Epoch::from(6),
            }),

            params: EraParams {
                epoch_size_slots: 432000,
                slot_length: TimeMs::new(1000),
                era_name: EraName::Allegra,
            },
        },
        Summary {
            start: Bound {
                time_ms: TimeMs::from(2592000000),
                slot: Slot::from(950400),
                epoch: Epoch::from(6),
            },
            end: Some(Bound {
                time_ms: TimeMs::from(3024000000),
                slot: Slot::from(1382400),
                epoch: Epoch::from(7),
            }),

            params: EraParams {
                epoch_size_slots: 432000,
                slot_length: TimeMs::new(1000),
                era_name: EraName::Mary,
            },
        },
        Summary {
            start: Bound {
                time_ms: TimeMs::from(3024000000),
                slot: Slot::from(1382400),
                epoch: Epoch::from(7),
            },
            end: Some(Bound {
                time_ms: TimeMs::from(5184000000),
                slot: Slot::from(3542400),
                epoch: Epoch::from(12),
            }),

            params: EraParams {
                epoch_size_slots: 432000,
                slot_length: TimeMs::new(1000),
                era_name: EraName::Alonzo,
            },
        },
        Summary {
            start: Bound {
                time_ms: TimeMs::from(5184000000),
                slot: Slot::from(3542400),
                epoch: Epoch::from(12),
            },
            end: Some(Bound {
                time_ms: TimeMs::from(70416000000),
                slot: Slot::from(68774400),
                epoch: Epoch::from(163),
            }),

            params: EraParams {
                epoch_size_slots: 432000,
                slot_length: TimeMs::new(1000),
                era_name: EraName::Babbage,
            },
        },
        Summary {
            start: Bound {
                time_ms: TimeMs::from(70416000000),
                slot: Slot::from(68774400),
                epoch: Epoch::from(163),
            },
            end: None,
            params: EraParams {
                epoch_size_slots: 432000,
                slot_length: TimeMs::new(1000),
                era_name: EraName::Conway,
            },
        },
    ];

    EraHistory::new(&eras, PREPROD_GLOBAL_PARAMETERS.stability_window)
});

/// Era history for Preview retrieved with:
///
/// ```bash
/// curl -X POST "https://preview.koios.rest/api/v1/ogmios"
///  -H 'accept: application/json'
///  -H 'content-type: application/json'
///  -d '{"jsonrpc":"2.0","method":"queryLedgerState/eraSummaries"}' | jq -c '.result'
/// ```
///
pub static PREVIEW_ERA_HISTORY: LazyLock<EraHistory> = LazyLock::new(|| {
    let eras: [Summary; 7] = [
        Summary {
            start: Bound {
                time_ms: TimeMs::from(0),
                slot: Slot::from(0),
                epoch: Epoch::from(0),
            },
            end: Some(Bound {
                time_ms: TimeMs::from(0),
                slot: Slot::from(0),
                epoch: Epoch::from(0),
            }),
            params: EraParams {
                epoch_size_slots: 4320,
                slot_length: TimeMs::new(20000),
                era_name: EraName::Byron,
            },
        },
        Summary {
            start: Bound {
                time_ms: TimeMs::from(0),
                slot: Slot::from(0),
                epoch: Epoch::from(0),
            },
            end: Some(Bound {
                time_ms: TimeMs::from(0),
                slot: Slot::from(0),
                epoch: Epoch::from(0),
            }),
            params: EraParams {
                epoch_size_slots: 86400,
                slot_length: TimeMs::new(1000),
                era_name: EraName::Shelley,
            },
        },
        Summary {
            start: Bound {
                time_ms: TimeMs::from(0),
                slot: Slot::from(0),
                epoch: Epoch::from(5),
            },
            end: Some(Bound {
                time_ms: TimeMs::from(0),
                slot: Slot::from(0),
                epoch: Epoch::from(0),
            }),

            params: EraParams {
                epoch_size_slots: 86400,
                slot_length: TimeMs::new(1000),
                era_name: EraName::Allegra,
            },
        },
        Summary {
            start: Bound {
                time_ms: TimeMs::from(0),
                slot: Slot::from(0),
                epoch: Epoch::from(0),
            },
            end: Some(Bound {
                time_ms: TimeMs::from(0),
                slot: Slot::from(0),
                epoch: Epoch::from(0),
            }),

            params: EraParams {
                epoch_size_slots: 86400,
                slot_length: TimeMs::new(1000),
                era_name: EraName::Mary,
            },
        },
        Summary {
            start: Bound {
                time_ms: TimeMs::from(0),
                slot: Slot::from(0),
                epoch: Epoch::from(0),
            },
            end: Some(Bound {
                time_ms: TimeMs::from(259200000),
                slot: Slot::from(259200),
                epoch: Epoch::from(3),
            }),

            params: EraParams {
                epoch_size_slots: 86400,
                slot_length: TimeMs::new(1000),
                era_name: EraName::Alonzo,
            },
        },
        Summary {
            start: Bound {
                time_ms: TimeMs::from(259200000),
                slot: Slot::from(259200),
                epoch: Epoch::from(3),
            },
            end: Some(Bound {
                time_ms: TimeMs::from(55814400000),
                slot: Slot::from(55814400),
                epoch: Epoch::from(646),
            }),

            params: EraParams {
                epoch_size_slots: 86400,
                slot_length: TimeMs::new(1000),
                era_name: EraName::Babbage,
            },
        },
        Summary {
            start: Bound {
                time_ms: TimeMs::from(55814400000),
                slot: Slot::from(55814400),
                epoch: Epoch::from(646),
            },
            end: None,

            params: EraParams {
                epoch_size_slots: 86400,
                slot_length: TimeMs::new(1000),
                era_name: EraName::Conway,
            },
        },
    ];

    EraHistory::new(&eras, Slot::from(25920))
});

/// A default era history for testnets
///
/// This default `EraHistory` contains a single era which covers 1000 epochs,
/// with a slot length of 1 second and epoch size of 86400 slots.
pub static TESTNET_ERA_HISTORY: LazyLock<EraHistory> = LazyLock::new(|| {
    let eras: [Summary; 1] = [Summary {
        start: Bound {
            time_ms: TimeMs::from(0),
            slot: Slot::from(0),
            epoch: Epoch::from(0),
        },
        end: None,

        params: EraParams {
            epoch_size_slots: 86400, // one day
            slot_length: TimeMs::new(1000),
            era_name: EraName::Conway,
        },
    }];

    EraHistory::new(&eras, TESTNET_GLOBAL_PARAMETERS.stability_window)
});

/// Error type for era history file operations
#[derive(Debug, thiserror::Error)]
pub enum EraHistoryFileError {
    #[error("Failed to open era history file: {0}")]
    FileOpenError(#[from] std::io::Error),
    #[error("Failed to parse era history JSON: {0}")]
    JsonParseError(#[from] serde_json::Error),
}

/// Load an `EraHistory` from a JSON file.
///
/// # Arguments
///
/// * `path` - Path to the JSON file containing era history data
///
/// # Returns
///
/// Returns a Result containing the `EraHistory` if successful, or an `EraHistoryFileError` if the file
/// cannot be read or parsed.
///
/// # Example
///
/// ```no_run
/// use amaru_kernel::load_era_history_from_file;
/// use std::path::Path;
///
/// let era_history = load_era_history_from_file(Path::new("era_history.json")).unwrap();
/// ```
pub fn load_era_history_from_file(path: &Path) -> Result<EraHistory, EraHistoryFileError> {
    let file = File::open(path).map_err(EraHistoryFileError::FileOpenError)?;
    let reader = BufReader::new(file);

    serde_json::from_reader(reader).map_err(EraHistoryFileError::JsonParseError)
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Bound {
    pub time_ms: TimeMs, // Milliseconds
    pub slot: Slot,
    pub epoch: Epoch,
}

#[cfg(test)]
impl Bound {
    fn genesis() -> Bound {
        Bound {
            time_ms: TimeMs::new(0),
            slot: Slot::new(0),
            epoch: Epoch::new(0),
        }
    }
}

impl<C> cbor::Encode<C> for Bound {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.array(3)?;
        self.time_ms.encode(e, ctx)?;
        self.slot.encode(e, ctx)?;
        self.epoch.encode(e, ctx)?;
        Ok(())
    }
}

impl<'b, C> cbor::Decode<'b, C> for Bound {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        cbor::heterogeneous_array(d, |d, assert_len| {
            assert_len(3)?;
            let time_ms = d.decode()?;
            let slot = d.decode()?;
            let epoch = d.decode_with(ctx)?;
            Ok(Bound {
                time_ms,
                slot,
                epoch,
            })
        })
    }
}

// The start is inclusive and the end is exclusive. In a valid EraHistory, the
// end of each era will equal the start of the next one.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Summary {
    pub start: Bound,
    pub end: Option<Bound>,
    pub params: EraParams,
}

impl Summary {
    /// Checks whether the current `Summary` ends after the given slot; In case
    /// where the Summary doesn't have any upper bound, then we check whether the
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

    /// Calculate a virtual end `Bound` given a time and the last era summary that we know of.
    ///
    /// **pre-condition**: the provided tip must be after (or equal) to the start of this era.
    fn calculate_end_bound(&self, tip: &Slot, stability_window: &Slot) -> Bound {
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

        Bound {
            time_ms: start.time_ms + foreseeable_slots * params.slot_length,
            slot: Slot::new(start.slot.as_u64() + foreseeable_slots),
            epoch: Epoch::new(max_foreseeable_epoch),
        }
    }
}

impl<C> cbor::Encode<C> for Summary {
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

impl<'b, C> cbor::Decode<'b, C> for Summary {
    fn decode(d: &mut cbor::Decoder<'b>, _ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let _ = d.array()?;
        let start = d.decode()?;
        let end = d.decode()?;
        let params = d.decode()?;
        d.skip()?;
        Ok(Summary { start, end, params })
    }
}

// A complete history of eras that have taken place.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub struct EraHistory {
    /// Number of slots for which the chain growth property guarantees at least k blocks.
    ///
    /// This is defined as 3 * k / f, where:
    ///
    /// - k is the network security parameter (mainnet = 2160);
    /// - f is the active slot coefficient (mainnet = 0.05);
    stability_window: Slot,

    /// Summary of each era boundaries.
    eras: Vec<Summary>,
}

impl<C> cbor::Encode<C> for EraHistory {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.begin_array()?;
        for s in &self.eras {
            s.encode(e, ctx)?;
        }
        e.end()?;
        Ok(())
    }
}

impl<'b> cbor::Decode<'b, Slot> for EraHistory {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut Slot) -> Result<Self, cbor::decode::Error> {
        let mut eras = vec![];
        let eras_iter: cbor::decode::ArrayIter<'_, '_, Summary> = d.array_iter()?;
        for era in eras_iter {
            eras.push(era?);
        }
        Ok(EraHistory {
            stability_window: *ctx,
            eras,
        })
    }
}

#[derive(Debug, PartialEq, Eq, thiserror::Error, serde::Serialize, serde::Deserialize)]
pub enum EraHistoryError {
    #[error("slot past time horizon")]
    PastTimeHorizon,
    #[error("invalid era history")]
    InvalidEraHistory,
    #[error("{0}")]
    SlotArithmetic(#[from] SlotArithmeticError),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EpochBounds {
    pub start: Slot,
    pub end: Option<Slot>,
}

// The last era in the provided EraHistory must end at the time horizon for accurate results. The
// horizon is the end of the epoch containing the end of the current era's safe zone relative to
// the current tip. Returns number of milliseconds elapsed since the system start time.
impl EraHistory {
    pub fn new(eras: &[Summary], stability_window: Slot) -> EraHistory {
        #[expect(clippy::panic)]
        if eras.is_empty() {
            panic!("EraHistory cannot be empty");
        }
        // TODO ensures only last era ends with Option
        EraHistory {
            stability_window,
            eras: eras.to_vec(),
        }
    }

    pub fn slot_to_posix_time(
        &self,
        slot: Slot,
        tip: Slot,
        system_start: TimeMs,
    ) -> Result<TimeMs, EraHistoryError> {
        let relative_time = self.slot_to_relative_time(slot, tip)?;

        Ok(relative_time + system_start)
    }

    pub fn slot_to_relative_time(&self, slot: Slot, tip: Slot) -> Result<TimeMs, EraHistoryError> {
        for era in &self.eras {
            if era.start.slot > slot {
                return Err(EraHistoryError::InvalidEraHistory);
            }

            if era.contains_slot(&slot, &tip, &self.stability_window) {
                return slot_to_relative_time(&slot, era);
            }
        }

        Err(EraHistoryError::PastTimeHorizon)
    }

    /// Unsafe version of `slot_to_relative_time` which doesn't check whether the slot is within a
    /// foreseeable horizon. Only use this when the slot is guaranteed to be in the past.
    pub fn slot_to_relative_time_unchecked_horizon(
        &self,
        slot: Slot,
    ) -> Result<TimeMs, EraHistoryError> {
        for era in &self.eras {
            if era.start.slot > slot {
                return Err(EraHistoryError::InvalidEraHistory);
            }

            if era.contains_slot_unchecked_horizon(&slot) {
                return slot_to_relative_time(&slot, era);
            }
        }

        Err(EraHistoryError::InvalidEraHistory)
    }

    pub fn slot_to_epoch(&self, slot: Slot, tip: Slot) -> Result<Epoch, EraHistoryError> {
        for era in &self.eras {
            if era.start.slot > slot {
                return Err(EraHistoryError::InvalidEraHistory);
            }

            if era.contains_slot(&slot, &tip, &self.stability_window) {
                return slot_to_epoch(&slot, era);
            }
        }

        Err(EraHistoryError::PastTimeHorizon)
    }

    pub fn slot_to_epoch_unchecked_horizon(&self, slot: Slot) -> Result<Epoch, EraHistoryError> {
        for era in &self.eras {
            if era.start.slot > slot {
                return Err(EraHistoryError::InvalidEraHistory);
            }

            if era.contains_slot_unchecked_horizon(&slot) {
                return slot_to_epoch(&slot, era);
            }
        }

        Err(EraHistoryError::InvalidEraHistory)
    }

    pub fn next_epoch_first_slot(&self, epoch: Epoch, tip: &Slot) -> Result<Slot, EraHistoryError> {
        for era in &self.eras {
            if era.start.epoch > epoch {
                return Err(EraHistoryError::InvalidEraHistory);
            }

            if era.contains_epoch(&epoch, tip, &self.stability_window) {
                let start_of_next_epoch = (epoch.as_u64() - era.start.epoch.as_u64() + 1)
                    * era.params.epoch_size_slots
                    + era.start.slot.as_u64();
                return Ok(Slot::new(start_of_next_epoch));
            }
        }
        Err(EraHistoryError::PastTimeHorizon)
    }

    /// Find the first epoch of the era from which this epoch belongs.
    pub fn era_first_epoch(&self, epoch: Epoch) -> Result<Epoch, EraHistoryError> {
        for era in &self.eras {
            // NOTE: This is okay. If there's no upper-bound to the era and the slot is after the
            // start of it, then necessarily the era's lower bound is what we're looking for.
            if era.contains_epoch_unchecked_horizon(&epoch) {
                return Ok(era.start.epoch);
            }
        }

        Err(EraHistoryError::InvalidEraHistory)
    }

    pub fn epoch_bounds(&self, epoch: Epoch) -> Result<EpochBounds, EraHistoryError> {
        for era in &self.eras {
            if era.start.epoch > epoch {
                return Err(EraHistoryError::InvalidEraHistory);
            }

            // NOTE: Unchecked horizon is okay here since in case there's no upper-bound, we'll
            // simply return a `None` epoch bounds as well.
            if era.contains_epoch_unchecked_horizon(&epoch) {
                let epochs_elapsed = epoch - era.start.epoch;
                let offset = era.start.slot;
                let slots_elapsed = epochs_elapsed * era.params.epoch_size_slots;
                let start = offset.offset_by(slots_elapsed);
                let end = offset.offset_by(era.params.epoch_size_slots + slots_elapsed);
                return Ok(EpochBounds {
                    start,
                    end: era.end.as_ref().map(|_| end),
                });
            }
        }

        Err(EraHistoryError::InvalidEraHistory)
    }

    /// Computes the relative slot in the epoch given an absolute slot.
    ///
    /// Returns the number of slots since the start of the epoch containing the given slot.
    ///
    /// # Errors
    ///
    /// Returns `EraHistoryError::PastTimeHorizon` if the slot is beyond the time horizon.
    /// Returns `EraHistoryError::InvalidEraHistory` if the era history is invalid.
    pub fn slot_in_epoch(&self, slot: Slot, tip: Slot) -> Result<Slot, EraHistoryError> {
        let epoch = self.slot_to_epoch(slot, tip)?;
        let bounds = self.epoch_bounds(epoch)?;
        let elapsed = slot.elapsed_from(bounds.start)?;
        Ok(Slot::new(elapsed))
    }

    /// Returns the era index (0-based) for the given slot.
    ///
    /// The era index should correspond to the position in the era history vector:
    /// - 0 = Byron
    /// - 1 = Shelley
    /// - 2 = Allegra
    /// - 3 = Mary
    /// - 4 = Alonzo
    /// - 5 = Babbage
    /// - 6 = Conway
    pub fn slot_to_era_index(&self, slot: Slot) -> Result<usize, EraHistoryError> {
        for (index, era) in self.eras.iter().enumerate() {
            if era.start.slot > slot {
                return Err(EraHistoryError::InvalidEraHistory);
            }

            // Check if slot is in this era: start <= slot < end (end is exclusive)
            let in_era = match &era.end {
                Some(end) => slot < end.slot,
                None => true, // Last era has no end bound
            };

            if in_era {
                return Ok(index);
            }
        }

        Err(EraHistoryError::InvalidEraHistory)
    }

    /// Compute the era tag (used for serializating) from a slot using the era history.
    pub fn slot_to_era_tag(&self, slot: Slot) -> Result<EraName, EraHistoryError> {
        let era_index = self.slot_to_era_index(slot)?;
        Ok(self.eras[era_index].params.era_name)
    }
}

/// Compute the time in milliseconds between the start of the system and the given slot.
///
/// **pre-condition**: the given summary must be the era containing that slot.
fn slot_to_relative_time(slot: &Slot, era: &Summary) -> Result<TimeMs, EraHistoryError> {
    let slots_elapsed = slot
        .elapsed_from(era.start.slot)
        .map_err(|_| EraHistoryError::InvalidEraHistory)?;
    let time_elapsed = era.params.slot_length * slots_elapsed;
    let relative_time = era.start.time_ms + time_elapsed;
    Ok(relative_time)
}

/// Compute the epoch corresponding to the given slot.
///
/// **pre-condition**: the given summary must be the era containing that slot.
fn slot_to_epoch(slot: &Slot, era: &Summary) -> Result<Epoch, EraHistoryError> {
    let slots_elapsed = slot
        .elapsed_from(era.start.slot)
        .map_err(|_| EraHistoryError::InvalidEraHistory)?;
    let epochs_elapsed = slots_elapsed / era.params.epoch_size_slots;
    let epoch_number = era.start.epoch + epochs_elapsed;
    Ok(epoch_number)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Epoch, PREPROD_ERA_HISTORY, Slot, any_era_name, any_network_name,
        load_era_history_from_file,
    };
    use proptest::{prelude::*, proptest};
    use std::{
        cmp::{max, min},
        env,
        fs::File,
        io::Write,
        path::Path,
        str::FromStr,
    };
    use test_case::test_case;

    prop_compose! {
        fn arbitrary_time_ms()(ms in 0u64..u64::MAX) -> TimeMs {
            TimeMs::new(ms)
        }
    }

    prop_compose! {
        fn arbitrary_bound()(time_ms in arbitrary_time_ms(), slot in any::<u32>(), epoch in any::<Epoch>()) -> Bound {
            Bound {
                time_ms, slot: Slot::new(slot as u64), epoch
            }
        }
    }
    prop_compose! {
        fn arbitrary_bound_for_epoch(epoch: Epoch)(time_ms in arbitrary_time_ms(), slot in any::<u32>()) -> Bound {
            Bound {
                time_ms, slot: Slot::new(slot as u64), epoch
            }
        }
    }
    prop_compose! {
        fn arbitrary_era_params()(epoch_size_slots in 1u64..65535u64, slot_length in 1u64..65535u64, era_name in any_era_name()) -> EraParams {
            EraParams {
                epoch_size_slots,
                slot_length: TimeMs::new(slot_length),
                era_name,
            }
        }
    }

    prop_compose! {
        fn arbitrary_summary()(
            b1 in any::<u16>(),
            b2 in any::<u16>(),
            params in arbitrary_era_params(),
        )(
            first_epoch in Just(min(b1, b2) as u64),
            last_epoch in Just(max(b1, b2) as u64),
            params in Just(params),
            start in arbitrary_bound_for_epoch(Epoch::from(max(b1, b2) as u64)),
        ) -> Summary {
            let epochs_elapsed = last_epoch - first_epoch;
            let slots_elapsed = epochs_elapsed * params.epoch_size_slots;
            let time_elapsed = slots_elapsed * params.slot_length;
            let end = Some(Bound {
                time_ms: start.time_ms + time_elapsed,
                slot: start.slot.offset_by(slots_elapsed),
                epoch: Epoch::from(last_epoch),
            });
            Summary { start, end, params }
        }
    }

    prop_compose! {
        // Generate an arbitrary list of ordered epochs where we might have a new era
        fn arbitrary_boundaries()(
            first_epoch in any::<u16>(),
            era_lengths in prop::collection::vec(1u64..1000, 1usize..32usize),
        ) -> Vec<u64> {
            let mut boundaries = vec![first_epoch as u64];
            for era_length in era_lengths {
                boundaries.push(boundaries.last().unwrap() + era_length);
            }
            boundaries
        }
    }

    prop_compose! {
        fn arbitrary_era_history()(
            boundaries in arbitrary_boundaries()
        )(
            stability_window in any::<u64>(),
            era_params in prop::collection::vec(arbitrary_era_params(), boundaries.len()),
            boundaries in Just(boundaries),
        ) -> EraHistory {
            let genesis = Bound::genesis();

            let mut prev_bound = genesis;

            // For each boundary, compute the time and slot for that epoch based on the era params and
            // construct a summary from the boundary pair
            let mut summaries = vec![];
            for (boundary, prev_era_params) in boundaries.iter().zip(era_params.iter()) {
                let epochs_elapsed = boundary - prev_bound.epoch.as_u64();
                let slots_elapsed = epochs_elapsed * prev_era_params.epoch_size_slots;
                let time_elapsed = slots_elapsed * prev_era_params.slot_length;
                let new_bound = Bound {
                    time_ms: prev_bound.time_ms + time_elapsed,
                    slot: prev_bound.slot.offset_by(slots_elapsed),
                    epoch: Epoch::new(*boundary),
                };

                summaries.push(Summary {
                    start: prev_bound,
                    end: if *boundary as usize == boundaries.len() {
                        None
                    } else {
                        Some(new_bound.clone())
                    },
                    params: prev_era_params.clone(),
                });

                prev_bound = new_bound;
            }

            EraHistory::new(&summaries, Slot::from(stability_window))
        }
    }

    fn default_params() -> EraParams {
        EraParams::new(86400, 1000, EraName::Conway).unwrap()
    }

    fn one_era() -> EraHistory {
        EraHistory {
            stability_window: Slot::new(25920),
            eras: vec![Summary {
                start: Bound {
                    time_ms: TimeMs::new(0),
                    slot: Slot::new(0),
                    epoch: Epoch::new(0),
                },
                end: None,
                params: default_params(),
            }],
        }
    }

    fn two_eras() -> EraHistory {
        EraHistory {
            stability_window: Slot::new(25920),
            eras: vec![
                Summary {
                    start: Bound {
                        time_ms: TimeMs::new(0),
                        slot: Slot::new(0),
                        epoch: Epoch::new(0),
                    },
                    end: Some(Bound {
                        time_ms: TimeMs::new(86400000),
                        slot: Slot::new(86400),
                        epoch: Epoch::new(1),
                    }),
                    params: default_params(),
                },
                Summary {
                    start: Bound {
                        time_ms: TimeMs::new(86400000),
                        slot: Slot::new(86400),
                        epoch: Epoch::new(1),
                    },
                    end: None,
                    params: default_params(),
                },
            ],
        }
    }

    #[test]
    fn slot_to_relative_time_within_horizon() {
        let eras = two_eras();
        assert_eq!(
            eras.slot_to_relative_time(Slot::new(172800), Slot::new(172800)),
            Ok(TimeMs::new(172800000))
        );
    }

    #[test]
    fn slot_to_time_fails_after_time_horizon() {
        let eras = two_eras();
        assert_eq!(
            eras.slot_to_relative_time(Slot::new(172800), Slot::new(100000)),
            Ok(TimeMs::new(172800000)),
            "point is right at the end of the epoch, tip is somewhere"
        );
        assert_eq!(
            eras.slot_to_relative_time(Slot::new(172801), Slot::new(86400)),
            Err(EraHistoryError::PastTimeHorizon),
            "point in the next epoch, but tip is way before the stability window (first slot)"
        );
        assert_eq!(
            eras.slot_to_relative_time(Slot::new(172801), Slot::new(100000)),
            Err(EraHistoryError::PastTimeHorizon),
            "point in the next epoch, but tip is way before the stability window (somewhere)"
        );
        assert_eq!(
            eras.slot_to_relative_time(Slot::new(172801), Slot::new(146880)),
            Ok(TimeMs::new(172801000)),
            "point in the next epoch, and tip right at the stability window limit"
        );
        assert_eq!(
            eras.slot_to_relative_time(Slot::new(172801), Slot::new(146879)),
            Err(EraHistoryError::PastTimeHorizon),
            "point in the next epoch, but tip right *before* the stability window limit"
        );
    }

    #[test]
    fn epoch_bounds_epoch_0() {
        let bounds = two_eras().epoch_bounds(Epoch::new(0)).unwrap();
        assert_eq!(bounds.start, Slot::new(0));
        assert_eq!(bounds.end, Some(Slot::new(86400)));
    }

    #[test]
    fn epoch_bounds_epoch_1() {
        let bounds = two_eras().epoch_bounds(Epoch::new(1)).unwrap();
        assert_eq!(bounds.start, Slot::new(86400));
        assert_eq!(bounds.end, None);
    }

    const MAINNET_SYSTEM_START: u64 = 1506203091000;

    #[test_case(0, 42, MAINNET_SYSTEM_START
        => Ok(MAINNET_SYSTEM_START);
        "first slot in the system, tip is irrelevant"
    )]
    #[test_case(1000, 42, MAINNET_SYSTEM_START
        => Ok(MAINNET_SYSTEM_START + 1000 * 1000);
        "one thousand slots after genesis, tip is irrelevant"
    )]
    #[test_case(172801, 0, MAINNET_SYSTEM_START
        => Err(EraHistoryError::PastTimeHorizon);
        "slot is at the next epcoh, but tip is at genesis"
    )]
    fn slot_to_posix(slot: u64, tip: u64, system_start: u64) -> Result<u64, EraHistoryError> {
        two_eras()
            .slot_to_posix_time(slot.into(), tip.into(), system_start.into())
            .map(|time_ms| time_ms.into())
    }

    #[test_case(0,          42 => Ok(0);
        "first slot in first epoch, tip irrelevant"
    )]
    #[test_case(48272,      42 => Ok(0);
        "slot anywhere in first epoch, tip irrelevant"
    )]
    #[test_case(86400,      42 => Ok(1);
        "first slot in second epoch, tip irrelevant"
    )]
    #[test_case(105437,     42 => Ok(1);
        "slot anywhere in second epoch, tip irrelevant"
    )]
    #[test_case(172801, 146879 => Err(EraHistoryError::PastTimeHorizon);
        "slot beyond first epoch (at the frontier), tip before stable area"
    )]
    #[test_case(200000, 146879 => Err(EraHistoryError::PastTimeHorizon);
        "slot beyond first epoch (anywhere), tip before stable area"
    )]
    #[test_case(200000, 146880 => Ok(2);
        "slot within third epoch, tip in stable area (lower frontier)"
    )]
    #[test_case(200000, 153129 => Ok(2);
        "slot within third epoch, tip in stable area (anywhere)"
    )]
    #[test_case(200000, 172800 => Ok(2);
        "slot within third epoch, tip in stable area (upper frontier)"
    )]
    #[test_case(260000, 146880 => Err(EraHistoryError::PastTimeHorizon);
        "slot far far away, tip in stable area (lower frontier)"
    )]
    #[test_case(260000, 153129 => Err(EraHistoryError::PastTimeHorizon);
        "slot far far away, tip in stable area (anywhere)"
    )]
    #[test_case(260000, 172800 => Err(EraHistoryError::PastTimeHorizon);
        "slot far far away, tip in stable area (upper frontier)"
    )]
    fn slot_to_epoch(slot: u64, tip: u64) -> Result<u64, EraHistoryError> {
        two_eras()
            .slot_to_epoch(Slot::new(slot), Slot::new(tip))
            .map(|epoch| epoch.into())
    }

    #[test_case(0 => Ok(0); "first slot in first epoch")]
    #[test_case(48272 => Ok(0); "slot anywhere in first epoch")]
    #[test_case(86400 => Ok(1); "first slot in second epoch")]
    #[test_case(105437 => Ok(1); "slot anywhere in second epoch")]
    #[test_case(172801 => Ok(2); "slot beyond first epoch (at the frontier)")]
    #[test_case(200000 => Ok(2); "slot within third epoch")]
    #[test_case(260000 => Ok(3); "slot far far away")]
    fn slot_to_epoch_unchecked_horizon(slot: u64) -> Result<u64, EraHistoryError> {
        two_eras()
            .slot_to_epoch_unchecked_horizon(Slot::new(slot))
            .map(|epoch| epoch.into())
    }

    #[test_case(0, 42 => Ok(86400); "fully known forecast (1), tip irrelevant")]
    #[test_case(1, 42 => Ok(172800); "fully known forecast (2), tip irrelevant")]
    #[test_case(2, 42 => Err(EraHistoryError::PastTimeHorizon);
        "far away forecast, tip before stable window (well before)"
    )]
    #[test_case(2, 146879 => Err(EraHistoryError::PastTimeHorizon);
        "far away forecast, tip before stable window (lower frontier)"
    )]
    #[test_case(2, 146880 => Ok(259200);
        "far away forecast, tip within stable window (lower frontier)"
    )]
    #[test_case(2, 201621 => Ok(259200);
        "far away forecast, tip within stable window (anywhere)"
    )]
    #[test_case(2, 259199 => Ok(259200);
        "far away forecast, tip within stable window (upper frontier)"
    )]
    fn next_epoch_first_slot(epoch: u64, tip: u64) -> Result<u64, EraHistoryError> {
        two_eras()
            .next_epoch_first_slot(Epoch::new(epoch), &Slot::new(tip))
            .map(|slot| slot.into())
    }

    #[test]
    fn slot_in_epoch_invalid_era_history() {
        // Create an invalid era history where the second era starts at a slot
        // that is earlier than the first era's start slot
        let invalid_eras = EraHistory {
            stability_window: Slot::new(129600),
            eras: vec![
                Summary {
                    start: Bound {
                        time_ms: TimeMs::new(100000),
                        slot: Slot::new(100),
                        epoch: Epoch::new(1),
                    },
                    end: Some(Bound {
                        time_ms: TimeMs::new(186400000),
                        slot: Slot::new(86500),
                        epoch: Epoch::new(2),
                    }),
                    params: default_params(),
                },
                Summary {
                    start: Bound {
                        time_ms: TimeMs::new(186400000),
                        slot: Slot::new(50), // This is invalid - earlier than first era's start
                        epoch: Epoch::new(2),
                    },
                    end: Some(Bound {
                        time_ms: TimeMs::new(272800000),
                        slot: Slot::new(86450),
                        epoch: Epoch::new(3),
                    }),
                    params: default_params(),
                },
            ],
        };

        let relative_slot = invalid_eras.slot_in_epoch(Slot::new(60), Slot::new(42));
        assert_eq!(relative_slot, Err(EraHistoryError::InvalidEraHistory));
    }

    #[test]
    fn slot_in_epoch_underflows_given_era_history_with_gaps() {
        // Create a custom era history with a gap between epochs
        let invalid_eras = EraHistory {
            stability_window: Slot::new(129600),
            eras: vec![
                Summary {
                    start: Bound {
                        time_ms: TimeMs::new(0),
                        slot: Slot::new(0),
                        epoch: Epoch::new(0),
                    },
                    end: Some(Bound {
                        time_ms: TimeMs::new(86400000),
                        slot: Slot::new(86400),
                        epoch: Epoch::new(1),
                    }),
                    params: default_params(),
                },
                Summary {
                    start: Bound {
                        time_ms: TimeMs::new(86400000),
                        slot: Slot::new(186400), // Gap of 100000 slots
                        epoch: Epoch::new(1),
                    },
                    end: Some(Bound {
                        time_ms: TimeMs::new(172800000),
                        slot: Slot::new(272800),
                        epoch: Epoch::new(2),
                    }),
                    params: default_params(),
                },
            ],
        };

        // A slot in epoch 1 but before the start of epoch 1's slots
        let problematic_slot = Slot::new(100000);

        let result = invalid_eras.slot_in_epoch(problematic_slot, Slot::new(42));
        assert_eq!(result, Err(EraHistoryError::InvalidEraHistory));
    }

    #[test]
    fn encode_era_history() {
        let eras = one_era();
        let buffer = cbor::to_vec(&eras).unwrap();
        assert_eq!(
            hex::encode(buffer),
            "9f9f83000000f6831a000151801b000000e8d4a5100007ffff"
        );
    }

    #[test]
    fn can_decode_bounds_with_unbounded_integer_slot() {
        // CBOR encoding for
        // [[[0, 0, 0], [0, 0, 0]],
        //  [[0, 0, 0], [0, 0, 0]],
        //  [[0, 0, 0], [0, 0, 0]],
        //  [[0, 0, 0], [0, 0, 0]],
        //  [[0, 0, 0], [259200000000000000, 259200, 3]],
        //  [[259200000000000000, 259200, 3], [55814400000000000000, 55814400, 646]]]
        let buffer = hex::decode("868283000000830000008283000000830000008283000000830000008283000000830000008283000000831b0398dd06d5c800001a0003f4800382831b0398dd06d5c800001a0003f4800383c2490306949515279000001a0353a900190286").unwrap();
        let eras: Vec<(Bound, Bound)> = cbor::decode(&buffer).unwrap();

        assert_eq!(eras[5].1.time_ms, TimeMs::new(55814400000));
    }

    #[test]
    fn scales_encoded_bounds_to_ms_precision() {
        // CBOR encoding for
        //   [259200000000000000, 259200, 3]
        let buffer = hex::decode("831b0398dd06d5c800001a0003f48003").unwrap();
        let bound: Bound = cbor::decode(&buffer)
            .expect("cannot decode '831b0398dd06d5c800001a0003f48003' as a Bound");

        assert_eq!(bound.time_ms, TimeMs::new(259200000));
    }

    #[test]
    fn cannot_decode_bounds_with_too_large_integer_value() {
        // CBOR encoding for
        // [558144000000000000001234567890000000000, 55814400, 646]
        let buffer =
            hex::decode("83c25101a3e69fd156bd141cccb9fb74768db4001a0353a900190286").unwrap();
        let result = cbor::decode::<Bound>(&buffer);
        assert!(result.is_err());
    }

    #[test]
    fn cannot_decode_bounds_with_invalid_tag() {
        // CBOR encoding for
        // [-558144000000000000001234567890000000001, 55814400, 646]
        let buffer =
            hex::decode("83c35101a3e69fd156bd141cccb9fb74768db4001a0353a900190286").unwrap();
        let result = cbor::decode::<Bound>(&buffer);
        assert!(result.is_err());
    }

    #[test]
    fn era_index_from_slot() {
        let eras = two_eras();
        assert_eq!(
            eras.slot_to_era_index(Slot::new(0)),
            Ok(0),
            "first slot in first era"
        );
        assert_eq!(
            eras.slot_to_era_index(Slot::new(48272)),
            Ok(0),
            "slot anywhere in first era"
        );
        assert_eq!(
            eras.slot_to_era_index(Slot::new(86399)),
            Ok(0),
            "last slot in first era"
        );
        assert_eq!(
            eras.slot_to_era_index(Slot::new(86400)),
            Ok(1),
            "first slot in second era"
        );
        assert_eq!(
            eras.slot_to_era_index(Slot::new(105437)),
            Ok(1),
            "slot anywhere in second era"
        );
        assert_eq!(
            eras.slot_to_era_index(Slot::new(172801)),
            Ok(1),
            "slot beyond first epoch in second era"
        );
        assert_eq!(
            eras.slot_to_era_index(Slot::new(200000)),
            Ok(1),
            "slot well into second era"
        );
    }

    proptest! {
        #[test]
        fn roundtrip_era_history(era_history in arbitrary_era_history()) {
            let buffer = cbor::to_vec(&era_history).unwrap();
            let decoded = cbor::decode_with(&buffer, &mut era_history.stability_window.clone()).unwrap();
            assert_eq!(era_history, decoded);
        }

        #[test]
        fn roundtrip_bounds(bound in arbitrary_bound()) {
            let buffer = cbor::to_vec(&bound).unwrap();
            let msg = format!("failed to decode {}", hex::encode(&buffer));
            let decoded = cbor::decode(&buffer).expect(&msg);
            assert_eq!(bound, decoded);
        }
    }

    proptest! {
        #[test]
        fn prop_can_parse_pretty_print_network_name(network in any_network_name()) {
            let name = format!("{}", network);
            assert_eq!(
                FromStr::from_str(&name),
                Ok(network),
            )
        }
    }

    #[test]
    fn can_compute_slot_to_epoch_for_preprod() {
        let era_history = &*PREPROD_ERA_HISTORY;
        assert_eq!(
            Epoch::from(4),
            era_history
                .slot_to_epoch_unchecked_horizon(Slot::from(86400))
                .unwrap()
        );
        assert_eq!(
            Epoch::from(11),
            era_history
                .slot_to_epoch_unchecked_horizon(Slot::from(3542399))
                .unwrap()
        );
        assert_eq!(
            Epoch::from(12),
            era_history
                .slot_to_epoch_unchecked_horizon(Slot::from(3542400))
                .unwrap()
        );
    }

    #[test]
    fn can_compute_next_epoch_first_slot_for_preprod() {
        let era_history = &*PREPROD_ERA_HISTORY;
        let some_tip = Slot::from(96486650);
        assert_eq!(
            era_history.next_epoch_first_slot(Epoch::from(3), &some_tip),
            Ok(Slot::from(86400))
        );
        assert_eq!(
            era_history.next_epoch_first_slot(Epoch::from(114), &some_tip),
            Ok(Slot::from(48038400))
        );
        assert_eq!(
            era_history.next_epoch_first_slot(Epoch::from(150), &some_tip),
            Ok(Slot::from(63590400))
        );
    }

    #[test]
    fn test_era_history_json_serialization() {
        let original_era_history = &*PREPROD_ERA_HISTORY;

        let mut temp_file_path = env::temp_dir();
        temp_file_path.push("test_era_history.json");

        let json_data = serde_json::to_string_pretty(original_era_history)
            .expect("Failed to serialize EraHistory to JSON");

        let mut file = File::create(&temp_file_path).expect("Failed to create temporary file");

        file.write_all(json_data.as_bytes())
            .expect("Failed to write JSON data to file");

        let loaded_era_history = load_era_history_from_file(temp_file_path.as_path())
            .expect("Failed to load EraHistory from file");

        assert_eq!(
            *original_era_history, loaded_era_history,
            "Era histories don't match"
        );

        std::fs::remove_file(temp_file_path).ok();
    }

    #[test]
    fn test_era_history_file_open_error() {
        let non_existent_path = Path::new("non_existent_file.json");

        let result = load_era_history_from_file(non_existent_path);

        match result {
            Err(EraHistoryFileError::FileOpenError(_)) => {
                // This is the expected error type
            }
            _ => panic!("Expected FileOpenError, got {:?}", result),
        }
    }

    #[test]
    fn test_era_history_json_parse_error() {
        let mut temp_file_path = env::temp_dir();
        temp_file_path.push("invalid_era_history.json");

        let invalid_json = r#"{ "eras": [invalid json] }"#;

        let mut file = File::create(&temp_file_path).expect("Failed to create temporary file");

        file.write_all(invalid_json.as_bytes())
            .expect("Failed to write invalid JSON data to file");

        let result = load_era_history_from_file(temp_file_path.as_path());

        match result {
            Err(EraHistoryFileError::JsonParseError(_)) => {
                // This is the expected error type
            }
            _ => panic!("Expected JsonParseError, got {:?}", result),
        }

        std::fs::remove_file(temp_file_path).ok();
    }
}
