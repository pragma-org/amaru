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

mod common;

use std::collections::BTreeMap;

use amaru_kernel::{
    BlockHeight, EraName, HeaderHash, MultiEraBlock, Point, RawBlock, cardano::era_name::ERA_NAMES, parse_block_header,
    protocol_version::PROTOCOL_VERSION_10,
};
pub use common::*;

/// Conformance against the CBOR corpus published by <https://github.com/r2rationality/cardano-cbor-dataset>.
/// Fetch it with:
/// ```text
/// make fetch-cbor-dataset
/// ```
///
/// If the data is missing, the test just prints a warning.
///
/// Set `AMARU_TEST_REPORT_DIRECTORY=<directory>` to also write the per-rule counters and the failures to a JSON file
/// that will be used to report the Amaru node conformance.
#[test]
fn test_cbor_dataset() {
    let Some(test_configuration) = TestConfiguration::create(PROTOCOL_VERSION_10).unwrap() else {
        return;
    };
    let mut test_results = TestResults::new();

    for (rule, round_trip) in RULES {
        let results = check_rule(&test_configuration, rule, round_trip).expect("failed to check the rule");
        test_results.append(results);
        if *rule == "block" {
            for sample in test_configuration.read_tests_for(rule).unwrap() {
                if sample.category() == Category::Valid {
                    let mut bytes = vec![0x82, EraName::Conway as u8];
                    bytes.extend_from_slice(&sample.read_bytes().unwrap());
                    MultiEraBlock::decode(&bytes).unwrap_or_else(|error| panic!("{sample}: {error}"));
                }
            }
        }
    }

    // Don't fail the test if some failures are acknowledged in the acknowledged-failures.toml file.
    test_results.acknowledge_failures(&test_configuration);

    report(&test_configuration, &test_results).expect("failed to report the results");

    // Fail the test if some failures are not acknowledged in the acknowledged-failures.toml file
    // or if some acknowledgements are now obsolete.
    check_unacknowledged_failures(&test_results);
}

#[test]
fn test_multi_era_blocks() {
    #[derive(serde::Deserialize)]
    struct Fixture {
        slot: u64,
        height: BlockHeight,
        hash: HeaderHash,
        parent: HeaderHash,
        cbor: String,
    }

    #[derive(serde::Deserialize)]
    struct Fixtures {
        blocks: BTreeMap<EraName, Fixture>,
    }

    let fixtures: Fixtures = serde_json::from_str(include_str!("data/multi-era-blocks.json")).unwrap();
    assert_eq!(fixtures.blocks.keys().copied().collect::<Vec<_>>(), ERA_NAMES[..EraName::Conway as usize]);
    for (era, fixture) in fixtures.blocks {
        let bytes = hex::decode(fixture.cbor).unwrap();
        let raw = RawBlock::from(bytes.as_slice());
        let block = raw.decode_multi_era().unwrap_or_else(|error| panic!("{era}: {error}"));
        let header = block.header();
        assert_eq!(header.era(), era);
        assert_eq!(header.block_variant(), era as u8);
        assert_eq!(header.point(21_600).unwrap(), Point::Specific(fixture.slot.into(), fixture.hash, fixture.height));
        assert_eq!(block.parent_hash(), Some(fixture.parent));
        assert_eq!(parse_block_header(&bytes).unwrap(), header);
    }
}
