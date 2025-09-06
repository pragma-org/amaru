// Copyright 2025 PRAGMA
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

use crate::protocol_parameters::{
    GlobalParameters, MAINNET_GLOBAL_PARAMETERS, PREPROD_GLOBAL_PARAMETERS,
    PREVIEW_GLOBAL_PARAMETERS, TESTNET_GLOBAL_PARAMETERS,
};
use amaru_slot_arithmetic::TimeMs;
use pallas_addresses::Network;
use std::{fs::File, io::BufReader, path::Path, sync::LazyLock};

pub use amaru_slot_arithmetic::{Bound, Epoch, EraHistory, EraParams, Slot, Summary};

/// Era history for Mainnet retrieved with:
///
/// ```bash
/// curl -X POST "https://mainnet.koios.rest/api/v1/ogmios"
///  -H 'accept: application/json'
///  -H 'content-type: application/json'
///  -d '{"jsonrpc":"2.0","method":"queryLedgerState/eraSummaries"}' | jq -c '.result'
/// ```
///
static MAINNET_ERA_HISTORY: LazyLock<EraHistory> = LazyLock::new(|| {
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
                slot_length: 20000,
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
                slot_length: 1000,
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
                slot_length: 1000,
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
                slot_length: 1000,
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
                slot_length: 1000,
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
                slot_length: 1000,
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
                slot_length: 1000,
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
static PREPROD_ERA_HISTORY: LazyLock<EraHistory> = LazyLock::new(|| {
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
                slot_length: 20000,
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
                slot_length: 1000,
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
                slot_length: 1000,
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
                slot_length: 1000,
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
                slot_length: 1000,
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
                slot_length: 1000,
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
                slot_length: 1000,
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
static PREVIEW_ERA_HISTORY: LazyLock<EraHistory> = LazyLock::new(|| {
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
                slot_length: 20000,
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
                slot_length: 1000,
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
                slot_length: 1000,
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
                slot_length: 1000,
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
                slot_length: 1000,
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
                slot_length: 1000,
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
                slot_length: 1000,
            },
        },
    ];

    EraHistory::new(&eras, Slot::from(25920))
});

/// A default era history for testnets
///
/// This default `EraHistory` contains a single era which covers 1000 epochs,
/// with a slot length of 1 second and epoch size of 86400 slots.
static TESTNET_ERA_HISTORY: LazyLock<EraHistory> = LazyLock::new(|| {
    let eras: [Summary; 1] = [Summary {
        start: Bound {
            time_ms: TimeMs::from(0),
            slot: Slot::from(0),
            epoch: Epoch::from(0),
        },
        end: None,

        params: EraParams {
            epoch_size_slots: 86400, // one day
            slot_length: 1000,
        },
    }];

    EraHistory::new(&eras, PREVIEW_GLOBAL_PARAMETERS.stability_window)
});

impl From<NetworkName> for &EraHistory {
    fn from(value: NetworkName) -> Self {
        match value {
            NetworkName::Mainnet => &MAINNET_ERA_HISTORY,
            NetworkName::Preprod => &PREPROD_ERA_HISTORY,
            NetworkName::Preview => &PREVIEW_ERA_HISTORY,
            NetworkName::Testnet(_) => &TESTNET_ERA_HISTORY,
        }
    }
}

impl From<NetworkName> for &GlobalParameters {
    fn from(value: NetworkName) -> Self {
        match value {
            NetworkName::Mainnet => &MAINNET_GLOBAL_PARAMETERS,
            NetworkName::Preprod => &PREPROD_GLOBAL_PARAMETERS,
            NetworkName::Preview => &PREVIEW_GLOBAL_PARAMETERS,
            NetworkName::Testnet(_) => &TESTNET_GLOBAL_PARAMETERS,
        }
    }
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum NetworkName {
    Mainnet,
    Preprod,
    Preview,
    Testnet(u32),
}

impl std::fmt::Display for NetworkName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Mainnet => write!(f, "mainnet"),
            Self::Preprod => write!(f, "preprod"),
            Self::Preview => write!(f, "preview"),
            Self::Testnet(magic) => write!(f, "testnet:{}", magic),
        }
    }
}

impl std::str::FromStr for NetworkName {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "mainnet" => Ok(Self::Mainnet),
            "preprod" => Ok(Self::Preprod),
            "preview" => Ok(Self::Preview),
            _ => {
                let magic = s
                    .strip_prefix("testnet:")
                    .ok_or(format!("Invalid network name {}", s))?;

                magic
                    .parse::<u32>()
                    .map(NetworkName::Testnet)
                    .map_err(|e| e.to_string())
            }
        }
    }
}

impl From<NetworkName> for Network {
    fn from(value: NetworkName) -> Self {
        if value == NetworkName::Mainnet {
            Network::Mainnet
        } else {
            Network::Testnet
        }
    }
}

impl NetworkName {
    pub fn to_network_magic(self) -> u32 {
        match self {
            Self::Mainnet => 764824073,
            Self::Preprod => 1,
            Self::Preview => 2,
            Self::Testnet(magic) => magic,
        }
    }
}

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
/// use amaru_kernel::network::load_era_history_from_file;
/// use std::path::Path;
///
/// let era_history = load_era_history_from_file(Path::new("era_history.json")).unwrap();
/// ```
pub fn load_era_history_from_file(path: &Path) -> Result<EraHistory, EraHistoryFileError> {
    let file = File::open(path).map_err(EraHistoryFileError::FileOpenError)?;
    let reader = BufReader::new(file);

    serde_json::from_reader(reader).map_err(EraHistoryFileError::JsonParseError)
}

#[cfg(any(test, feature = "test-utils"))]
pub mod tests {
    use super::NetworkName::{self, *};
    use crate::Network;
    use proptest::{prelude::*, prop_oneof};

    pub fn any_network_name() -> impl Strategy<Value = NetworkName> {
        prop_oneof![
            Just(Mainnet),
            Just(Preprod),
            Just(Preview),
            (3..u32::MAX).prop_map(Testnet)
        ]
    }

    prop_compose! {
        pub fn any_network()(
            is_testnet in any::<bool>(),
        ) -> Network {
            if is_testnet {
                Network::Testnet
            } else {
                Network::Mainnet
            }
        }
    }

    #[cfg(test)]
    mod internal {
        use super::{super::EraHistoryFileError, any_network_name};
        use crate::network::{load_era_history_from_file, PREPROD_ERA_HISTORY};
        use amaru_slot_arithmetic::{Epoch, Slot};
        use proptest::proptest;
        use std::{env, fs::File, io::Write, path::Path, str::FromStr};

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
}
