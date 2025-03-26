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

pub use slot_arithmetic::{Bound, EraHistory, EraParams, Summary};
use std::fmt;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::OnceLock;

/// Epoch number in which the PreProd network transitioned to Shelley.
pub const PREPROD_SHELLEY_TRANSITION_EPOCH: usize = 4;

/// Era history for Preprod retrieved with:
///
/// ```bash
/// curl -X POST "https://preprod.koios.rest/api/v1/ogmios"
///  -H 'accept: application/json'
///  -H 'content-type: application/json'
///  -d '{"jsonrpc":"2.0","method":"queryLedgerState/eraSummaries"}' | jq -c '.result'
/// ```
///
const PREPROD_ERAS: [Summary; 7] = [
    Summary {
        start: Bound {
            time_ms: 0,
            slot: 0,
            epoch: 0,
        },
        end: Bound {
            time_ms: 1728000000,
            slot: 86400,
            epoch: 4,
        },
        params: EraParams {
            epoch_size_slots: 21600,
            slot_length: 20000,
        },
    },
    Summary {
        start: Bound {
            time_ms: 1728000000,
            slot: 86400,
            epoch: 4,
        },
        end: Bound {
            time_ms: 2160000000,
            slot: 518400,
            epoch: 5,
        },
        params: EraParams {
            epoch_size_slots: 432000,
            slot_length: 1000,
        },
    },
    Summary {
        start: Bound {
            time_ms: 2160000000,
            slot: 518400,
            epoch: 5,
        },
        end: Bound {
            time_ms: 2592000000,
            slot: 950400,
            epoch: 6,
        },

        params: EraParams {
            epoch_size_slots: 432000,
            slot_length: 1000,
        },
    },
    Summary {
        start: Bound {
            time_ms: 2592000000,
            slot: 950400,
            epoch: 6,
        },
        end: Bound {
            time_ms: 3024000000,
            slot: 1382400,
            epoch: 7,
        },

        params: EraParams {
            epoch_size_slots: 432000,
            slot_length: 1000,
        },
    },
    Summary {
        start: Bound {
            time_ms: 3024000000,
            slot: 1382400,
            epoch: 7,
        },
        end: Bound {
            time_ms: 5184000000,
            slot: 3542400,
            epoch: 12,
        },

        params: EraParams {
            epoch_size_slots: 432000,
            slot_length: 1000,
        },
    },
    Summary {
        start: Bound {
            time_ms: 5184000000,
            slot: 3542400,
            epoch: 12,
        },
        end: Bound {
            time_ms: 70416000000,
            slot: 68774400,
            epoch: 163,
        },

        params: EraParams {
            epoch_size_slots: 432000,
            slot_length: 1000,
        },
    },
    Summary {
        start: Bound {
            time_ms: 70416000000,
            slot: 68774400,
            epoch: 163,
        },
        end: Bound {
            time_ms: 89424000000,
            slot: 87782400,
            epoch: 207,
        },

        params: EraParams {
            epoch_size_slots: 432000,
            slot_length: 1000,
        },
    },
];

pub fn preprod_era_history() -> &'static EraHistory {
    static PREPROD_ERA_HISTORY: OnceLock<EraHistory> = OnceLock::new();
    PREPROD_ERA_HISTORY.get_or_init(|| EraHistory {
        eras: PREPROD_ERAS.to_vec(),
    })
}

/// A tesnet with a single era spanning 1000 epochs
const DEFAULT_TESTNET_ERAS: [Summary; 1] = [Summary {
    start: Bound {
        time_ms: 0,
        slot: 0,
        epoch: 0,
    },
    end: Bound {
        time_ms: 432000000000,
        slot: 432000000,
        epoch: 1000,
    },

    params: EraParams {
        epoch_size_slots: 432000,
        slot_length: 1000,
    },
}];

/// A default era history for testnets
///
/// This default `EraHistory` contains a single era which covers 1000 epochs,
/// with a slot length of 1 second and epoch size of 432000 slots.
pub fn testnet_default_era_history() -> &'static EraHistory {
    static TESTNET_ERA_HISTORY: OnceLock<EraHistory> = OnceLock::new();
    TESTNET_ERA_HISTORY.get_or_init(|| EraHistory {
        eras: DEFAULT_TESTNET_ERAS.to_vec(),
    })
}

#[allow(clippy::todo)]
impl From<NetworkName> for &EraHistory {
    fn from(value: NetworkName) -> Self {
        match value {
            NetworkName::Mainnet => todo!(),
            NetworkName::Preprod => preprod_era_history(),
            NetworkName::Preview => todo!(),
            NetworkName::Testnet(_) => testnet_default_era_history(),
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
#[derive(Debug)]
pub enum EraHistoryFileError {
    /// Error when opening the file
    FileOpenError(std::io::Error),
    /// Error when parsing the JSON content
    JsonParseError(serde_json::Error),
}

impl fmt::Display for EraHistoryFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FileOpenError(err) => write!(f, "Failed to open era history file: {}", err),
            Self::JsonParseError(err) => write!(f, "Failed to parse era history JSON: {}", err),
        }
    }
}

impl std::error::Error for EraHistoryFileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::FileOpenError(err) => Some(err),
            Self::JsonParseError(err) => Some(err),
        }
    }
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

#[cfg(test)]
mod tests {
    use crate::network::{load_era_history_from_file, preprod_era_history};

    use super::EraHistoryFileError;
    use super::NetworkName::{self, *};
    use proptest::prelude::*;
    use proptest::{prop_oneof, proptest};
    use std::env;
    use std::fs::File;
    use std::io::Write;
    use std::path::Path;
    use std::str::FromStr;

    fn any_network() -> impl Strategy<Value = NetworkName> {
        prop_oneof![
            Just(Mainnet),
            Just(Preprod),
            Just(Preview),
            (3..u32::MAX).prop_map(Testnet)
        ]
    }

    proptest! {
        #[test]
        fn prop_can_parse_pretty_print_network_name(network in any_network()) {
            let name = format!("{}", network);
            assert_eq!(
                FromStr::from_str(&name),
                Ok(network),
            )
        }
    }

    #[test]
    fn can_compute_slot_to_epoch_for_preprod() {
        assert_eq!(4, preprod_era_history().slot_to_epoch(86400).unwrap());
        assert_eq!(11, preprod_era_history().slot_to_epoch(3542399).unwrap());
        assert_eq!(12, preprod_era_history().slot_to_epoch(3542400).unwrap());
    }

    #[test]
    fn test_era_history_json_serialization() {
        let original_era_history = preprod_era_history();

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

        // Clean up the temporary file
        std::fs::remove_file(temp_file_path).ok();
    }

    #[test]
    fn test_era_history_file_open_error() {
        // Test with a non-existent file
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
        // Create a temporary file with invalid JSON
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

        // Clean up the temporary file
        std::fs::remove_file(temp_file_path).ok();
    }
}
