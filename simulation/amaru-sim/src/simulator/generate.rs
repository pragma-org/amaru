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

use proptest::prelude::*;
use rand::rngs::StdRng;
use rand::Rng;
use rand::SeedableRng;
use serde::Deserialize;
use serde_json::Result;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;

use super::bytes::Bytes;
use super::sync::ChainSyncMessage;
use slot_arithmetic::Slot;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Root {
    stake_pools: StakePools,
}

#[derive(Debug, Deserialize)]
struct StakePools {
    chains: Vec<Block>,
}

#[derive(Clone, PartialEq, Debug, Deserialize)]
struct Block {
    hash: String,
    header: String,
    height: u32,
    parent: Option<String>,
    slot: u64,
}

impl fmt::Display for Block {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "<hash: {}, slot: {}, height: {}>",
            &self.hash[..6],
            self.slot,
            self.height
        )
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct Chain {
    block: Block,
    children: Vec<Chain>,
}

fn read_chain_json(file_path: &str) -> String {
    fs::read_to_string(file_path).expect("Should have been able to read the chain.json file")
}

fn parse_json(bytes: &[u8]) -> Result<Vec<Block>> {
    let result: Root = serde_json::from_slice(bytes)?;
    Ok(result.stake_pools.chains)
}

fn recreate_chain(blocks: Vec<Block>) -> Chain {
    match blocks.as_slice() {
        [] => panic!("recreate_tree: no blocks"),
        [head, tail @ ..] => {
            assert!(head.parent.is_none(), "first block has a parent");
            let children = recreate_children(&head.hash, tail);
            Chain {
                block: head.clone(),
                children,
            }
        }
    }
}

fn recreate_children(parent_hash: &String, blocks: &[Block]) -> Vec<Chain> {
    let mut siblings = Vec::new();
    let mut used_indices = BTreeSet::new();

    for (i, block) in blocks.iter().enumerate() {
        if !used_indices.contains(&i) && block.parent.as_ref() == Some(parent_hash) {
            siblings.push(block.clone());
            used_indices.insert(i);
        }
    }

    let remaining_blocks: Vec<Block> = blocks
        .iter()
        .enumerate()
        .filter(|(i, _)| !used_indices.contains(i))
        .map(|(_, block)| block.clone())
        .collect();

    siblings
        .into_iter()
        .map(|block| {
            let children = recreate_children(&block.hash, &remaining_blocks);
            Chain { block, children }
        })
        .collect()
}

fn find_ancestors(chain: &Chain, target_hash: &str) -> Vec<Block> {
    let mut stack = vec![(chain, Vec::new())];

    while let Some((current_chain, mut ancestors)) = stack.pop() {
        if current_chain.block.hash == target_hash {
            return ancestors;
        }

        ancestors.push(current_chain.block.clone());

        for child in &current_chain.children {
            stack.push((child, ancestors.clone()));
        }
    }

    Vec::new() // Not found
}

fn generate_inputs_from_chain<R: Rng>(chain0: &Chain, rng: &mut R) -> Vec<ChainSyncMessage> {
    let mut messages = Vec::new();
    let mut msg_id = 0;

    messages.push(ChainSyncMessage::Fwd {
        msg_id,
        slot: Slot::from(chain0.block.slot),
        hash: Bytes {
            bytes: chain0.block.hash.clone().as_bytes().to_vec(),
        },
        header: Bytes {
            bytes: chain0.block.header.clone().as_bytes().to_vec(),
        },
    });
    msg_id += 1;

    fn walk_chain<R: Rng>(
        chain0: &Chain,
        chain: &Chain,
        messages: &mut Vec<ChainSyncMessage>,
        msg_id: &mut u64,
        rng: &mut R,
        visited: &mut BTreeMap<String, BTreeSet<usize>>,
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
                hash: Bytes {
                    bytes: chain.children[index].block.hash.clone().as_bytes().to_vec(),
                },
                header: Bytes {
                    bytes: chain.children[index]
                        .block
                        .header
                        .clone()
                        .as_bytes()
                        .to_vec(),
                },
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
        visited: &mut BTreeMap<String, BTreeSet<usize>>,
        height: u32,
    ) {
        let mut ancestors: Vec<Block> = find_ancestors(chain0, &chain.block.hash);
        ancestors.reverse();
        for ancestor in &ancestors {
            let ancestor_chain = find_chain(chain0, &ancestor.hash)
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
                    hash: Bytes {
                        bytes: ancestor.hash.clone().as_bytes().to_vec(),
                    },
                });
                *msg_id += 1;
                walk_chain(chain0, &ancestor_chain, messages, msg_id, rng, visited)
            }
        }
    }

    fn find_chain(chain: &Chain, hash: &String) -> Option<Chain> {
        let mut stack = vec![chain];

        while let Some(current) = stack.pop() {
            if current.block.hash == *hash {
                return Some(current.clone());
            }

            for child in &current.children {
                stack.push(child);
            }
        }

        None
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

pub fn generate_inputs<R: Rng>(rng: &mut R, file_path: &str) -> Result<Vec<ChainSyncMessage>> {
    let data = read_chain_json(file_path);
    match parse_json(data.as_bytes()) {
        Ok(blocks) => {
            let chain = recreate_chain(blocks);
            Ok(generate_inputs_from_chain(&chain, rng))
        }
        Err(err) => Err(err),
    }
}

pub fn generate_inputs_strategy(
    file_path: &str,
) -> impl Strategy<Value = Vec<ChainSyncMessage>> + use<'_> {
    any::<u64>().prop_map(|seed| {
        let mut rng = StdRng::seed_from_u64(seed);
        generate_inputs(&mut rng, file_path).unwrap()
    })
}

#[cfg(test)]
mod test {
    use crate::simulator::generate::*;

    #[test]
    fn test_ancestors() {
        let block_a = Block {
            hash: String::from("a"),
            header: String::from(""),
            height: 0,
            parent: None,
            slot: 0,
        };

        let block_b = Block {
            hash: String::from("b"),
            header: String::from(""),
            height: 1,
            parent: Some(String::from("a")),
            slot: 1,
        };

        let block_c = Block {
            hash: String::from("c"),
            header: String::from(""),
            height: 2,
            parent: Some(String::from("b")),
            slot: 2,
        };

        let block_d = Block {
            hash: String::from("d"),
            header: String::from(""),
            height: 1,
            parent: Some(String::from("a")),
            slot: 3,
        };

        let chain = Chain {
            block: block_a.clone(),
            children: vec![
                Chain {
                    block: block_b.clone(),
                    children: vec![Chain {
                        block: block_c.clone(),
                        children: Vec::new(),
                    }],
                },
                Chain {
                    block: block_d,
                    children: vec![],
                },
            ],
        };
        assert_eq!(find_ancestors(&chain, "a"), Vec::<Block>::new());
        assert_eq!(
            find_ancestors(&chain, "c"),
            vec![block_a.clone(), block_b.clone()]
        );
        assert_eq!(find_ancestors(&chain, "d"), vec![block_a]);
    }

    #[test]
    fn test_recreate_chain() {
        let data = read_chain_json("tests/data/chain.json");
        match parse_json(data.as_bytes()) {
            Ok(blocks) => {
                let chain = recreate_chain(blocks);
                println!("{}", draw_chain(&chain));
            }
            Err(e) => eprintln!("Error parsing JSON: {}", e),
        }
    }

    fn draw_chain(chain: &Chain) -> String {
        let lines = draw(chain);
        lines.join("\n")
    }

    fn draw(chain: &Chain) -> Vec<String> {
        let mut result = vec![format!("{}", chain.block)];
        result.extend(draw_subchains(&chain.children));
        result
    }

    fn draw_subchains(chains: &[Chain]) -> Vec<String> {
        if chains.is_empty() {
            return vec![];
        }

        let mut result = Vec::new();
        for (i, chain) in chains.iter().enumerate() {
            if i == 0 {
                result.push("|".to_string());
                result.extend(shift("`- ", "   ", &draw(chain)));
            } else {
                result.push("|".to_string());
                result.extend(shift("+- ", "|  ", &draw(chain)));
            }
        }
        result
    }

    fn shift(first: &str, other: &str, lines: &[String]) -> Vec<String> {
        let mut result = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            if i == 0 {
                result.push(format!("{}{}", first, line));
            } else {
                result.push(format!("{}{}", other, line));
            }
        }
        result
    }

    #[test]
    fn test_generate_inputs() {
        let data = read_chain_json("tests/data/chain.json");
        match parse_json(data.as_bytes()) {
            Ok(blocks) => {
                let seed = 1234;
                let mut rng = StdRng::seed_from_u64(seed);
                let chain = recreate_chain(blocks);
                let inputs = generate_inputs_from_chain(&chain, &mut rng);
                let expected = vec![
                    ChainSyncMessage::Fwd {
                        msg_id: 0,
                        slot: Slot::from(31),
                        hash: Bytes {
                            bytes: "2487bd".as_bytes().to_vec(),
                        },
                        header: Bytes {
                            bytes: "828a0118".as_bytes().to_vec(),
                        },
                    },
                    ChainSyncMessage::Fwd {
                        msg_id: 1,
                        slot: Slot::from(38),
                        hash: Bytes {
                            bytes: "4fcd1d".as_bytes().to_vec(),
                        },
                        header: Bytes {
                            bytes: "828a0218".as_bytes().to_vec(),
                        },
                    },
                    ChainSyncMessage::Fwd {
                        msg_id: 2,
                        slot: Slot::from(41),
                        hash: Bytes {
                            bytes: "739307".as_bytes().to_vec(),
                        },
                        header: Bytes {
                            bytes: "828a0318".as_bytes().to_vec(),
                        },
                    },
                    ChainSyncMessage::Fwd {
                        msg_id: 3,
                        slot: Slot::from(55),
                        hash: Bytes {
                            bytes: "726ef3".as_bytes().to_vec(),
                        },
                        header: Bytes {
                            bytes: "828a0418".as_bytes().to_vec(),
                        },
                    },
                    ChainSyncMessage::Fwd {
                        msg_id: 4,
                        slot: Slot::from(93),
                        hash: Bytes {
                            bytes: "597ea6".as_bytes().to_vec(),
                        },
                        header: Bytes {
                            bytes: "828a0518".as_bytes().to_vec(),
                        },
                    },
                    ChainSyncMessage::Fwd {
                        msg_id: 5,
                        slot: Slot::from(121),
                        hash: Bytes {
                            bytes: "bfa96c".as_bytes().to_vec(),
                        },
                        header: Bytes {
                            bytes: "828a0618".as_bytes().to_vec(),
                        },
                    },
                    ChainSyncMessage::Fwd {
                        msg_id: 6,
                        slot: Slot::from(142),
                        hash: Bytes {
                            bytes: "64565f".as_bytes().to_vec(),
                        },
                        header: Bytes {
                            bytes: "828a0718".as_bytes().to_vec(),
                        },
                    },
                    ChainSyncMessage::Fwd {
                        msg_id: 7,
                        slot: Slot::from(188),
                        hash: Bytes {
                            bytes: "bd41b1".as_bytes().to_vec(),
                        },
                        header: Bytes {
                            bytes: "828a0818".as_bytes().to_vec(),
                        },
                    },
                    ChainSyncMessage::Bck {
                        msg_id: 8,
                        slot: Slot::from(142),
                        hash: Bytes {
                            bytes: "64565f".as_bytes().to_vec(),
                        },
                    },
                    ChainSyncMessage::Fwd {
                        msg_id: 9,
                        slot: Slot::from(187),
                        hash: Bytes {
                            bytes: "66c90f".as_bytes().to_vec(),
                        },
                        header: Bytes {
                            bytes: "828a0818".as_bytes().to_vec(),
                        },
                    },
                    ChainSyncMessage::Fwd {
                        msg_id: 10,
                        slot: Slot::from(204),
                        hash: Bytes {
                            bytes: "3dcc0a".as_bytes().to_vec(),
                        },
                        header: Bytes {
                            bytes: "828a0918".as_bytes().to_vec(),
                        },
                    },
                    ChainSyncMessage::Fwd {
                        msg_id: 11,
                        slot: Slot::from(225),
                        hash: Bytes {
                            bytes: "1900c6".as_bytes().to_vec(),
                        },
                        header: Bytes {
                            bytes: "828a0a18".as_bytes().to_vec(),
                        },
                    },
                    ChainSyncMessage::Fwd {
                        msg_id: 12,
                        slot: Slot::from(272),
                        hash: Bytes {
                            bytes: "38fbcc".as_bytes().to_vec(),
                        },
                        header: Bytes {
                            bytes: "828a0b19".as_bytes().to_vec(),
                        },
                    },
                    ChainSyncMessage::Fwd {
                        msg_id: 13,
                        slot: Slot::from(309),
                        hash: Bytes {
                            bytes: "a7bafd".as_bytes().to_vec(),
                        },
                        header: Bytes {
                            bytes: "828a0c19".as_bytes().to_vec(),
                        },
                    },
                    ChainSyncMessage::Fwd {
                        msg_id: 14,
                        slot: Slot::from(314),
                        hash: Bytes {
                            bytes: "a45d60".as_bytes().to_vec(),
                        },
                        header: Bytes {
                            bytes: "828a0d19".as_bytes().to_vec(),
                        },
                    },
                    ChainSyncMessage::Fwd {
                        msg_id: 15,
                        slot: Slot::from(343),
                        hash: Bytes {
                            bytes: "75114e".as_bytes().to_vec(),
                        },
                        header: Bytes {
                            bytes: "828a0e19".as_bytes().to_vec(),
                        },
                    },
                    ChainSyncMessage::Fwd {
                        msg_id: 16,
                        slot: Slot::from(345),
                        hash: Bytes {
                            bytes: "5401ea".as_bytes().to_vec(),
                        },
                        header: Bytes {
                            bytes: "828a0f19".as_bytes().to_vec(),
                        },
                    },
                    ChainSyncMessage::Fwd {
                        msg_id: 17,
                        slot: Slot::from(374),
                        hash: Bytes {
                            bytes: "dec730".as_bytes().to_vec(),
                        },
                        header: Bytes {
                            bytes: "828a1019".as_bytes().to_vec(),
                        },
                    },
                    ChainSyncMessage::Fwd {
                        msg_id: 18,
                        slot: Slot::from(454),
                        hash: Bytes {
                            bytes: "05c824".as_bytes().to_vec(),
                        },
                        header: Bytes {
                            bytes: "828a1119".as_bytes().to_vec(),
                        },
                    },
                    ChainSyncMessage::Fwd {
                        msg_id: 19,
                        slot: Slot::from(473),
                        hash: Bytes {
                            bytes: "b967de".as_bytes().to_vec(),
                        },
                        header: Bytes {
                            bytes: "828a1219".as_bytes().to_vec(),
                        },
                    },
                    ChainSyncMessage::Fwd {
                        msg_id: 20,
                        slot: Slot::from(492),
                        hash: Bytes {
                            bytes: "42c0a5".as_bytes().to_vec(),
                        },
                        header: Bytes {
                            bytes: "828a1319".as_bytes().to_vec(),
                        },
                    },
                    ChainSyncMessage::Fwd {
                        msg_id: 21,
                        slot: Slot::from(521),
                        hash: Bytes {
                            bytes: "fdd5fb".as_bytes().to_vec(),
                        },
                        header: Bytes {
                            bytes: "828a1419".as_bytes().to_vec(),
                        },
                    },
                    ChainSyncMessage::Fwd {
                        msg_id: 22,
                        slot: Slot::from(535),
                        hash: Bytes {
                            bytes: "03409d".as_bytes().to_vec(),
                        },
                        header: Bytes {
                            bytes: "828a1519".as_bytes().to_vec(),
                        },
                    },
                    ChainSyncMessage::Fwd {
                        msg_id: 23,
                        slot: Slot::from(543),
                        hash: Bytes {
                            bytes: "5d3cbe".as_bytes().to_vec(),
                        },
                        header: Bytes {
                            bytes: "828a1619".as_bytes().to_vec(),
                        },
                    },
                    ChainSyncMessage::Fwd {
                        msg_id: 24,
                        slot: Slot::from(592),
                        hash: Bytes {
                            bytes: "6f582e".as_bytes().to_vec(),
                        },
                        header: Bytes {
                            bytes: "828a1719".as_bytes().to_vec(),
                        },
                    },
                    ChainSyncMessage::Fwd {
                        msg_id: 25,
                        slot: Slot::from(608),
                        hash: Bytes {
                            bytes: "387d52".as_bytes().to_vec(),
                        },
                        header: Bytes {
                            bytes: "828a1818".as_bytes().to_vec(),
                        },
                    },
                    ChainSyncMessage::Fwd {
                        msg_id: 26,
                        slot: Slot::from(611),
                        hash: Bytes {
                            bytes: "fe7f70".as_bytes().to_vec(),
                        },
                        header: Bytes {
                            bytes: "828a1819".as_bytes().to_vec(),
                        },
                    },
                    ChainSyncMessage::Fwd {
                        msg_id: 27,
                        slot: Slot::from(648),
                        hash: Bytes {
                            bytes: "b30bf2".as_bytes().to_vec(),
                        },
                        header: Bytes {
                            bytes: "828a181a".as_bytes().to_vec(),
                        },
                    },
                    ChainSyncMessage::Fwd {
                        msg_id: 28,
                        slot: Slot::from(665),
                        hash: Bytes {
                            bytes: "03d2a8".as_bytes().to_vec(),
                        },
                        header: Bytes {
                            bytes: "828a181b".as_bytes().to_vec(),
                        },
                    },
                    ChainSyncMessage::Fwd {
                        msg_id: 29,
                        slot: Slot::from(670),
                        hash: Bytes {
                            bytes: "525647".as_bytes().to_vec(),
                        },
                        header: Bytes {
                            bytes: "828a181c".as_bytes().to_vec(),
                        },
                    },
                    ChainSyncMessage::Fwd {
                        msg_id: 30,
                        slot: Slot::from(694),
                        hash: Bytes {
                            bytes: "65e5aa".as_bytes().to_vec(),
                        },
                        header: Bytes {
                            bytes: "828a181d".as_bytes().to_vec(),
                        },
                    },
                    ChainSyncMessage::Fwd {
                        msg_id: 31,
                        slot: Slot::from(703),
                        hash: Bytes {
                            bytes: "8bd65a".as_bytes().to_vec(),
                        },
                        header: Bytes {
                            bytes: "828a181e".as_bytes().to_vec(),
                        },
                    },
                    ChainSyncMessage::Fwd {
                        msg_id: 32,
                        slot: Slot::from(729),
                        hash: Bytes {
                            bytes: "a8e8db".as_bytes().to_vec(),
                        },
                        header: Bytes {
                            bytes: "828a181f".as_bytes().to_vec(),
                        },
                    },
                    ChainSyncMessage::Fwd {
                        msg_id: 33,
                        slot: Slot::from(748),
                        hash: Bytes {
                            bytes: "3d9b4c".as_bytes().to_vec(),
                        },
                        header: Bytes {
                            bytes: "828a1820".as_bytes().to_vec(),
                        },
                    },
                    ChainSyncMessage::Fwd {
                        msg_id: 34,
                        slot: Slot::from(766),
                        hash: Bytes {
                            bytes: "ed6142".as_bytes().to_vec(),
                        },
                        header: Bytes {
                            bytes: "828a1821".as_bytes().to_vec(),
                        },
                    },
                    ChainSyncMessage::Fwd {
                        msg_id: 35,
                        slot: Slot::from(777),
                        hash: Bytes {
                            bytes: "b4d194".as_bytes().to_vec(),
                        },
                        header: Bytes {
                            bytes: "828a1822".as_bytes().to_vec(),
                        },
                    },
                    ChainSyncMessage::Fwd {
                        msg_id: 36,
                        slot: Slot::from(782),
                        hash: Bytes {
                            bytes: "58be40".as_bytes().to_vec(),
                        },
                        header: Bytes {
                            bytes: "828a1823".as_bytes().to_vec(),
                        },
                    },
                    ChainSyncMessage::Fwd {
                        msg_id: 37,
                        slot: Slot::from(798),
                        hash: Bytes {
                            bytes: "b0f841".as_bytes().to_vec(),
                        },
                        header: Bytes {
                            bytes: "828a1824".as_bytes().to_vec(),
                        },
                    },
                    ChainSyncMessage::Fwd {
                        msg_id: 38,
                        slot: Slot::from(825),
                        hash: Bytes {
                            bytes: "2d628d".as_bytes().to_vec(),
                        },
                        header: Bytes {
                            bytes: "828a1825".as_bytes().to_vec(),
                        },
                    },
                    ChainSyncMessage::Fwd {
                        msg_id: 39,
                        slot: Slot::from(827),
                        hash: Bytes {
                            bytes: "8c54fe".as_bytes().to_vec(),
                        },
                        header: Bytes {
                            bytes: "828a1826".as_bytes().to_vec(),
                        },
                    },
                    ChainSyncMessage::Fwd {
                        msg_id: 40,
                        slot: Slot::from(833),
                        hash: Bytes {
                            bytes: "1c0936".as_bytes().to_vec(),
                        },
                        header: Bytes {
                            bytes: "828a1827".as_bytes().to_vec(),
                        },
                    },
                    ChainSyncMessage::Fwd {
                        msg_id: 41,
                        slot: Slot::from(854),
                        hash: Bytes {
                            bytes: "c5a715".as_bytes().to_vec(),
                        },
                        header: Bytes {
                            bytes: "828a1828".as_bytes().to_vec(),
                        },
                    },
                    ChainSyncMessage::Fwd {
                        msg_id: 42,
                        slot: Slot::from(861),
                        hash: Bytes {
                            bytes: "e2fdff".as_bytes().to_vec(),
                        },
                        header: Bytes {
                            bytes: "828a1829".as_bytes().to_vec(),
                        },
                    },
                    ChainSyncMessage::Fwd {
                        msg_id: 43,
                        slot: Slot::from(863),
                        hash: Bytes {
                            bytes: "00f025".as_bytes().to_vec(),
                        },
                        header: Bytes {
                            bytes: "828a182a".as_bytes().to_vec(),
                        },
                    },
                    ChainSyncMessage::Fwd {
                        msg_id: 44,
                        slot: Slot::from(874),
                        hash: Bytes {
                            bytes: "5bb6b1".as_bytes().to_vec(),
                        },
                        header: Bytes {
                            bytes: "828a182b".as_bytes().to_vec(),
                        },
                    },
                    ChainSyncMessage::Fwd {
                        msg_id: 45,
                        slot: Slot::from(915),
                        hash: Bytes {
                            bytes: "2a348c".as_bytes().to_vec(),
                        },
                        header: Bytes {
                            bytes: "828a182c".as_bytes().to_vec(),
                        },
                    },
                    ChainSyncMessage::Fwd {
                        msg_id: 46,
                        slot: Slot::from(949),
                        hash: Bytes {
                            bytes: "218266".as_bytes().to_vec(),
                        },
                        header: Bytes {
                            bytes: "828a182d".as_bytes().to_vec(),
                        },
                    },
                    ChainSyncMessage::Fwd {
                        msg_id: 47,
                        slot: Slot::from(951),
                        hash: Bytes {
                            bytes: "dc6018".as_bytes().to_vec(),
                        },
                        header: Bytes {
                            bytes: "828a182e".as_bytes().to_vec(),
                        },
                    },
                    ChainSyncMessage::Fwd {
                        msg_id: 48,
                        slot: Slot::from(990),
                        hash: Bytes {
                            bytes: "fcb4a5".as_bytes().to_vec(),
                        },
                        header: Bytes {
                            bytes: "828a182f".as_bytes().to_vec(),
                        },
                    },
                ];
                for (got, expect) in inputs.into_iter().zip(expected) {
                    assert_eq!(format!("{:?}", got), format!("{:?}", expect))
                }
            }
            Err(e) => eprintln!("Error parsing JSON: {}", e),
        }
    }
}
