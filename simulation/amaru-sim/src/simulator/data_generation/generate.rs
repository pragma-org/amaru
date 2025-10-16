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

use pure_stage::Instant;
use rand::Rng;
use serde_json::Result;
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use crate::echo::Envelope;

use crate::simulator::bytes::Bytes;
use crate::simulator::data_generation::base_generators::generate_arrival_times;
use crate::simulator::data_generation::chain::{Block, Chain};
use crate::simulator::{Entry, NodeConfig};
use crate::sync::ChainSyncMessage;
use amaru_slot_arithmetic::Slot;

fn generate_inputs_from_chain<R: Rng>(chain0: &Chain, rng: &mut R) -> Vec<ChainSyncMessage> {
    let mut messages = Vec::new();
    let mut msg_id = 0;

    messages.push(ChainSyncMessage::Fwd {
        msg_id,
        slot: Slot::from(chain0.block.slot),
        hash: chain0.block.hash.clone(),
        header: chain0.block.header.clone(),
    });
    msg_id += 1;

    fn walk_chain<R: Rng>(
        chain0: &Chain,
        chain: &Chain,
        messages: &mut Vec<ChainSyncMessage>,
        msg_id: &mut u64,
        rng: &mut R,
        visited: &mut BTreeMap<Bytes, BTreeSet<usize>>,
    ) {
        let already_visited: BTreeSet<usize> = visited
            .get(&chain.block.hash)
            .unwrap_or(&BTreeSet::new())
            .clone();

        let not_visited: Vec<usize> = (0..chain.children.len())
            .collect::<BTreeSet<usize>>()
            .difference(&already_visited)
            .cloned()
            .collect();

        if chain.children.is_empty() || not_visited.is_empty() {
            backtrack(
                chain0,
                chain,
                messages,
                msg_id,
                rng,
                visited,
                chain.block.height,
            );
            // println!("visited: {:?}", visited);
        } else {
            let random_not_visited_index = rng.random_range(0..not_visited.len());
            let index = not_visited[random_not_visited_index];
            // println!(
            //     "Fwd {} {}",
            //     &chain.children[index].block.hash.as_str()[..6],
            //     &chain.children[index].block.height
            // );
            messages.push(ChainSyncMessage::Fwd {
                msg_id: *msg_id,
                slot: Slot::from(chain.children[index].block.slot),
                hash: chain.children[index].block.hash.clone(),
                header: chain.children[index].block.header.clone(),
            });
            *msg_id += 1;
            // println!("visited, {} -> {}", &chain.block.hash.as_str()[..6], index);
            let mut already_visited: BTreeSet<usize> = visited
                .get(&chain.block.hash)
                .unwrap_or(&BTreeSet::new())
                .clone();
            let _ = already_visited.insert(index);
            let _ = visited.insert(chain.block.hash.clone(), already_visited.clone());
            // println!("visited: {:?}", visited);
            walk_chain(
                chain0,
                &chain.children[index],
                messages,
                msg_id,
                rng,
                visited,
            );
        }
    }

    fn backtrack<R: Rng>(
        chain0: &Chain,
        chain: &Chain,
        messages: &mut Vec<ChainSyncMessage>,
        msg_id: &mut u64,
        rng: &mut R,
        visited: &mut BTreeMap<Bytes, BTreeSet<usize>>,
        height: u32,
    ) {
        let mut ancestors: Vec<Block> = chain0.find_ancestors(&chain.block.hash);
        ancestors.reverse();
        for ancestor in &ancestors {
            let ancestor_chain = chain0
                .find(&ancestor.hash)
                .expect("ancestor has to be in the original chain");
            // Only go back to ancestors that have unvisited children that have the same or higher
            // height.
            // println!("backtrack, visited set: {:?}", visited.get(&ancestor.hash));
            // println!(
            //     "backtrack, {}: {}, visisted: {}",
            //     &ancestor.hash.as_str()[..6],
            //     ancestor_chain.children.len(),
            //     visited.get(&ancestor.hash).unwrap_or(&BTreeSet::new()).len()
            // );
            if ancestor_chain.children.len()
                != visited
                    .get(&ancestor.hash)
                    .unwrap_or(&BTreeSet::new())
                    .len()
                && ancestor_chain
                    .children
                    .clone()
                    .into_iter()
                    .any(|child| child.block.height >= height)
            {
                // println!("Bck {} {}", &ancestor.hash.as_str()[..6], &ancestor.height);
                messages.push(ChainSyncMessage::Bck {
                    msg_id: *msg_id,
                    slot: Slot::from(ancestor.slot),
                    hash: ancestor.hash.clone(),
                });
                *msg_id += 1;
                walk_chain(chain0, &ancestor_chain, messages, msg_id, rng, visited)
            }
        }
    }

    walk_chain(
        chain0,
        chain0,
        &mut messages,
        &mut msg_id,
        rng,
        &mut BTreeMap::new(),
    );
    messages
}

pub fn generate_inputs<R: Rng>(rng: &mut R, file_path: &PathBuf) -> Result<Vec<ChainSyncMessage>> {
    let chain = Chain::from_file(file_path)?;
    Ok(generate_inputs_from_chain(&chain, rng))
}

pub fn generate_entries<R: Rng>(
    node_config: &NodeConfig,
    start_time: Instant,
    mean_millis: f64,
) -> impl Fn(&mut R) -> Vec<Reverse<Entry<ChainSyncMessage>>> + use<'_, R> {
    let file_path = node_config.block_tree_file.clone();
    move |rng| {
        let mut entries: Vec<Reverse<Entry<ChainSyncMessage>>> = vec![];
        for client in 1..=node_config.number_of_upstream_peers {
            let messages = generate_inputs(rng, &file_path)
                .expect("Failed to generate inputs from chain file");
            let arrival_times =
                generate_arrival_times(start_time, mean_millis)(messages.len(), rng);
            entries.extend(
                messages
                    .into_iter()
                    .enumerate()
                    .map(|(idx, msg)| {
                        Reverse(Entry {
                            arrival_time: arrival_times[idx],
                            envelope: Envelope {
                                src: "c".to_owned() + &client.to_string(),
                                dest: "n1".to_string(),
                                body: msg,
                            },
                        })
                    })
                    .collect::<Vec<_>>(),
            );
        }
        entries
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use rand::SeedableRng;
    use rand::rngs::StdRng;
    use std::path::Path;

    #[test]
    fn test_generate_inputs() {
        let chain = Chain::from_file(&Path::new("tests/data/chain.json").into()).unwrap();
        let mut rng = StdRng::seed_from_u64(1234);
        let inputs = generate_inputs_from_chain(&chain, &mut rng);

        let expected = vec![
            ChainSyncMessage::Fwd {
                msg_id: 0,
                slot: Slot::from(31),
                hash: Bytes {
                    bytes: hex::decode("2487bd".as_bytes()).unwrap(),
                },
                header: Bytes {
                    bytes: hex::decode("828a0118".as_bytes()).unwrap(),
                },
            },
            ChainSyncMessage::Fwd {
                msg_id: 1,
                slot: Slot::from(38),
                hash: Bytes {
                    bytes: hex::decode("4fcd1d".as_bytes()).unwrap(),
                },
                header: Bytes {
                    bytes: hex::decode("828a0218".as_bytes()).unwrap(),
                },
            },
            ChainSyncMessage::Fwd {
                msg_id: 2,
                slot: Slot::from(41),
                hash: Bytes {
                    bytes: hex::decode("739307".as_bytes()).unwrap(),
                },
                header: Bytes {
                    bytes: hex::decode("828a0318".as_bytes()).unwrap(),
                },
            },
            ChainSyncMessage::Fwd {
                msg_id: 3,
                slot: Slot::from(55),
                hash: Bytes {
                    bytes: hex::decode("726ef3".as_bytes()).unwrap(),
                },
                header: Bytes {
                    bytes: hex::decode("828a0418".as_bytes()).unwrap(),
                },
            },
            ChainSyncMessage::Fwd {
                msg_id: 4,
                slot: Slot::from(93),
                hash: Bytes {
                    bytes: hex::decode("597ea6".as_bytes()).unwrap(),
                },
                header: Bytes {
                    bytes: hex::decode("828a0518".as_bytes()).unwrap(),
                },
            },
            ChainSyncMessage::Fwd {
                msg_id: 5,
                slot: Slot::from(121),
                hash: Bytes {
                    bytes: hex::decode("bfa96c".as_bytes()).unwrap(),
                },
                header: Bytes {
                    bytes: hex::decode("828a0618".as_bytes()).unwrap(),
                },
            },
            ChainSyncMessage::Fwd {
                msg_id: 6,
                slot: Slot::from(142),
                hash: Bytes {
                    bytes: hex::decode("64565f".as_bytes()).unwrap(),
                },
                header: Bytes {
                    bytes: hex::decode("828a0718".as_bytes()).unwrap(),
                },
            },
            ChainSyncMessage::Fwd {
                msg_id: 7,
                slot: Slot::from(188),
                hash: Bytes {
                    bytes: hex::decode("bd41b1".as_bytes()).unwrap(),
                },
                header: Bytes {
                    bytes: hex::decode("828a0818".as_bytes()).unwrap(),
                },
            },
            ChainSyncMessage::Bck {
                msg_id: 8,
                slot: Slot::from(142),
                hash: Bytes {
                    bytes: hex::decode("64565f".as_bytes()).unwrap(),
                },
            },
            ChainSyncMessage::Fwd {
                msg_id: 9,
                slot: Slot::from(187),
                hash: Bytes {
                    bytes: hex::decode("66c90f".as_bytes()).unwrap(),
                },
                header: Bytes {
                    bytes: hex::decode("828a0818".as_bytes()).unwrap(),
                },
            },
            ChainSyncMessage::Fwd {
                msg_id: 10,
                slot: Slot::from(204),
                hash: Bytes {
                    bytes: hex::decode("3dcc0a".as_bytes()).unwrap(),
                },
                header: Bytes {
                    bytes: hex::decode("828a0918".as_bytes()).unwrap(),
                },
            },
            ChainSyncMessage::Fwd {
                msg_id: 11,
                slot: Slot::from(225),
                hash: Bytes {
                    bytes: hex::decode("1900c6".as_bytes()).unwrap(),
                },
                header: Bytes {
                    bytes: hex::decode("828a0a18".as_bytes()).unwrap(),
                },
            },
            ChainSyncMessage::Fwd {
                msg_id: 12,
                slot: Slot::from(272),
                hash: Bytes {
                    bytes: hex::decode("38fbcc".as_bytes()).unwrap(),
                },
                header: Bytes {
                    bytes: hex::decode("828a0b19".as_bytes()).unwrap(),
                },
            },
            ChainSyncMessage::Fwd {
                msg_id: 13,
                slot: Slot::from(309),
                hash: Bytes {
                    bytes: hex::decode("a7bafd".as_bytes()).unwrap(),
                },
                header: Bytes {
                    bytes: hex::decode("828a0c19".as_bytes()).unwrap(),
                },
            },
            ChainSyncMessage::Fwd {
                msg_id: 14,
                slot: Slot::from(314),
                hash: Bytes {
                    bytes: hex::decode("a45d60".as_bytes()).unwrap(),
                },
                header: Bytes {
                    bytes: hex::decode("828a0d19".as_bytes()).unwrap(),
                },
            },
            ChainSyncMessage::Fwd {
                msg_id: 15,
                slot: Slot::from(343),
                hash: Bytes {
                    bytes: hex::decode("75114e".as_bytes()).unwrap(),
                },
                header: Bytes {
                    bytes: hex::decode("828a0e19".as_bytes()).unwrap(),
                },
            },
            ChainSyncMessage::Fwd {
                msg_id: 16,
                slot: Slot::from(345),
                hash: Bytes {
                    bytes: hex::decode("5401ea".as_bytes()).unwrap(),
                },
                header: Bytes {
                    bytes: hex::decode("828a0f19".as_bytes()).unwrap(),
                },
            },
            ChainSyncMessage::Fwd {
                msg_id: 17,
                slot: Slot::from(374),
                hash: Bytes {
                    bytes: hex::decode("dec730".as_bytes()).unwrap(),
                },
                header: Bytes {
                    bytes: hex::decode("828a1019".as_bytes()).unwrap(),
                },
            },
            ChainSyncMessage::Fwd {
                msg_id: 18,
                slot: Slot::from(454),
                hash: Bytes {
                    bytes: hex::decode("05c824".as_bytes()).unwrap(),
                },
                header: Bytes {
                    bytes: hex::decode("828a1119".as_bytes()).unwrap(),
                },
            },
            ChainSyncMessage::Fwd {
                msg_id: 19,
                slot: Slot::from(473),
                hash: Bytes {
                    bytes: hex::decode("b967de".as_bytes()).unwrap(),
                },
                header: Bytes {
                    bytes: hex::decode("828a1219".as_bytes()).unwrap(),
                },
            },
            ChainSyncMessage::Fwd {
                msg_id: 20,
                slot: Slot::from(492),
                hash: Bytes {
                    bytes: hex::decode("42c0a5".as_bytes()).unwrap(),
                },
                header: Bytes {
                    bytes: hex::decode("828a1319".as_bytes()).unwrap(),
                },
            },
            ChainSyncMessage::Fwd {
                msg_id: 21,
                slot: Slot::from(521),
                hash: Bytes {
                    bytes: hex::decode("fdd5fb".as_bytes()).unwrap(),
                },
                header: Bytes {
                    bytes: hex::decode("828a1419".as_bytes()).unwrap(),
                },
            },
            ChainSyncMessage::Fwd {
                msg_id: 22,
                slot: Slot::from(535),
                hash: Bytes {
                    bytes: hex::decode("03409d".as_bytes()).unwrap(),
                },
                header: Bytes {
                    bytes: hex::decode("828a1519".as_bytes()).unwrap(),
                },
            },
            ChainSyncMessage::Fwd {
                msg_id: 23,
                slot: Slot::from(543),
                hash: Bytes {
                    bytes: hex::decode("5d3cbe".as_bytes()).unwrap(),
                },
                header: Bytes {
                    bytes: hex::decode("828a1619".as_bytes()).unwrap(),
                },
            },
            ChainSyncMessage::Fwd {
                msg_id: 24,
                slot: Slot::from(592),
                hash: Bytes {
                    bytes: hex::decode("6f582e".as_bytes()).unwrap(),
                },
                header: Bytes {
                    bytes: hex::decode("828a1719".as_bytes()).unwrap(),
                },
            },
            ChainSyncMessage::Fwd {
                msg_id: 25,
                slot: Slot::from(608),
                hash: Bytes {
                    bytes: hex::decode("387d52".as_bytes()).unwrap(),
                },
                header: Bytes {
                    bytes: hex::decode("828a1818".as_bytes()).unwrap(),
                },
            },
            ChainSyncMessage::Fwd {
                msg_id: 26,
                slot: Slot::from(611),
                hash: Bytes {
                    bytes: hex::decode("fe7f70".as_bytes()).unwrap(),
                },
                header: Bytes {
                    bytes: hex::decode("828a1819".as_bytes()).unwrap(),
                },
            },
            ChainSyncMessage::Fwd {
                msg_id: 27,
                slot: Slot::from(648),
                hash: Bytes {
                    bytes: hex::decode("b30bf2".as_bytes()).unwrap(),
                },
                header: Bytes {
                    bytes: hex::decode("828a181a".as_bytes()).unwrap(),
                },
            },
            ChainSyncMessage::Fwd {
                msg_id: 28,
                slot: Slot::from(665),
                hash: Bytes {
                    bytes: hex::decode("03d2a8".as_bytes()).unwrap(),
                },
                header: Bytes {
                    bytes: hex::decode("828a181b".as_bytes()).unwrap(),
                },
            },
            ChainSyncMessage::Fwd {
                msg_id: 29,
                slot: Slot::from(670),
                hash: Bytes {
                    bytes: hex::decode("525647".as_bytes()).unwrap(),
                },
                header: Bytes {
                    bytes: hex::decode("828a181c".as_bytes()).unwrap(),
                },
            },
            ChainSyncMessage::Fwd {
                msg_id: 30,
                slot: Slot::from(694),
                hash: Bytes {
                    bytes: hex::decode("65e5aa".as_bytes()).unwrap(),
                },
                header: Bytes {
                    bytes: hex::decode("828a181d".as_bytes()).unwrap(),
                },
            },
            ChainSyncMessage::Fwd {
                msg_id: 31,
                slot: Slot::from(703),
                hash: Bytes {
                    bytes: hex::decode("8bd65a".as_bytes()).unwrap(),
                },
                header: Bytes {
                    bytes: hex::decode("828a181e".as_bytes()).unwrap(),
                },
            },
            ChainSyncMessage::Fwd {
                msg_id: 32,
                slot: Slot::from(729),
                hash: Bytes {
                    bytes: hex::decode("a8e8db".as_bytes()).unwrap(),
                },
                header: Bytes {
                    bytes: hex::decode("828a181f".as_bytes()).unwrap(),
                },
            },
            ChainSyncMessage::Fwd {
                msg_id: 33,
                slot: Slot::from(748),
                hash: Bytes {
                    bytes: hex::decode("3d9b4c".as_bytes()).unwrap(),
                },
                header: Bytes {
                    bytes: hex::decode("828a1820".as_bytes()).unwrap(),
                },
            },
            ChainSyncMessage::Fwd {
                msg_id: 34,
                slot: Slot::from(766),
                hash: Bytes {
                    bytes: hex::decode("ed6142".as_bytes()).unwrap(),
                },
                header: Bytes {
                    bytes: hex::decode("828a1821".as_bytes()).unwrap(),
                },
            },
            ChainSyncMessage::Fwd {
                msg_id: 35,
                slot: Slot::from(777),
                hash: Bytes {
                    bytes: hex::decode("b4d194".as_bytes()).unwrap(),
                },
                header: Bytes {
                    bytes: hex::decode("828a1822".as_bytes()).unwrap(),
                },
            },
            ChainSyncMessage::Fwd {
                msg_id: 36,
                slot: Slot::from(782),
                hash: Bytes {
                    bytes: hex::decode("58be40".as_bytes()).unwrap(),
                },
                header: Bytes {
                    bytes: hex::decode("828a1823".as_bytes()).unwrap(),
                },
            },
            ChainSyncMessage::Fwd {
                msg_id: 37,
                slot: Slot::from(798),
                hash: Bytes {
                    bytes: hex::decode("b0f841".as_bytes()).unwrap(),
                },
                header: Bytes {
                    bytes: hex::decode("828a1824".as_bytes()).unwrap(),
                },
            },
            ChainSyncMessage::Fwd {
                msg_id: 38,
                slot: Slot::from(825),
                hash: Bytes {
                    bytes: hex::decode("2d628d".as_bytes()).unwrap(),
                },
                header: Bytes {
                    bytes: hex::decode("828a1825".as_bytes()).unwrap(),
                },
            },
            ChainSyncMessage::Fwd {
                msg_id: 39,
                slot: Slot::from(827),
                hash: Bytes {
                    bytes: hex::decode("8c54fe".as_bytes()).unwrap(),
                },
                header: Bytes {
                    bytes: hex::decode("828a1826".as_bytes()).unwrap(),
                },
            },
            ChainSyncMessage::Fwd {
                msg_id: 40,
                slot: Slot::from(833),
                hash: Bytes {
                    bytes: hex::decode("1c0936".as_bytes()).unwrap(),
                },
                header: Bytes {
                    bytes: hex::decode("828a1827".as_bytes()).unwrap(),
                },
            },
            ChainSyncMessage::Fwd {
                msg_id: 41,
                slot: Slot::from(854),
                hash: Bytes {
                    bytes: hex::decode("c5a715".as_bytes()).unwrap(),
                },
                header: Bytes {
                    bytes: hex::decode("828a1828".as_bytes()).unwrap(),
                },
            },
            ChainSyncMessage::Fwd {
                msg_id: 42,
                slot: Slot::from(861),
                hash: Bytes {
                    bytes: hex::decode("e2fdff".as_bytes()).unwrap(),
                },
                header: Bytes {
                    bytes: hex::decode("828a1829".as_bytes()).unwrap(),
                },
            },
            ChainSyncMessage::Fwd {
                msg_id: 43,
                slot: Slot::from(863),
                hash: Bytes {
                    bytes: hex::decode("00f025".as_bytes()).unwrap(),
                },
                header: Bytes {
                    bytes: hex::decode("828a182a".as_bytes()).unwrap(),
                },
            },
            ChainSyncMessage::Fwd {
                msg_id: 44,
                slot: Slot::from(874),
                hash: Bytes {
                    bytes: hex::decode("5bb6b1".as_bytes()).unwrap(),
                },
                header: Bytes {
                    bytes: hex::decode("828a182b".as_bytes()).unwrap(),
                },
            },
            ChainSyncMessage::Fwd {
                msg_id: 45,
                slot: Slot::from(915),
                hash: Bytes {
                    bytes: hex::decode("2a348c".as_bytes()).unwrap(),
                },
                header: Bytes {
                    bytes: hex::decode("828a182c".as_bytes()).unwrap(),
                },
            },
            ChainSyncMessage::Fwd {
                msg_id: 46,
                slot: Slot::from(949),
                hash: Bytes {
                    bytes: hex::decode("218266".as_bytes()).unwrap(),
                },
                header: Bytes {
                    bytes: hex::decode("828a182d".as_bytes()).unwrap(),
                },
            },
            ChainSyncMessage::Fwd {
                msg_id: 47,
                slot: Slot::from(951),
                hash: Bytes {
                    bytes: hex::decode("dc6018".as_bytes()).unwrap(),
                },
                header: Bytes {
                    bytes: hex::decode("828a182e".as_bytes()).unwrap(),
                },
            },
            ChainSyncMessage::Fwd {
                msg_id: 48,
                slot: Slot::from(990),
                hash: Bytes {
                    bytes: hex::decode("fcb4a5".as_bytes()).unwrap(),
                },
                header: Bytes {
                    bytes: hex::decode("828a182f".as_bytes()).unwrap(),
                },
            },
        ];
        for (got, expect) in inputs.into_iter().zip(expected) {
            assert_eq!(format!("{:?}", got), format!("{:?}", expect))
        }
    }
}
