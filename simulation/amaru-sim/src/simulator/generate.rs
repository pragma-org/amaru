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

use rand::Rng;
use serde::Deserialize;
use serde_json::Result;
use std::collections::{HashMap, HashSet};
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

fn read_chain_json() -> String {
    let file_path = "tests/data/chain.json";
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
    let mut remaining_blocks = blocks.to_vec();

    while let Some(block) = remaining_blocks.first() {
        if block.parent.as_ref() == Some(parent_hash) {
            siblings.push(block.clone());
            remaining_blocks.remove(0);
        } else {
            break;
        }
    }

    siblings
        .into_iter()
        .map(|block| {
            let children = recreate_children(&block.hash, &remaining_blocks);
            Chain { block, children }
        })
        .collect()
}

fn find_ancestors(chain: &Chain, target_hash: &str) -> Vec<Block> {
    fn helper(chain: &Chain, target_hash: &str, ancestors: &mut Vec<Block>) -> bool {
        if chain.block.hash == *target_hash {
            return true;
        }

        ancestors.push(chain.block.clone());

        for child in &chain.children {
            if helper(&child, target_hash, ancestors) {
                return true;
            }
        }

        ancestors.pop();

        false
    }
    let mut result = Vec::new();
    let _bool = helper(chain, target_hash, &mut result);
    result
}

fn generate_inputs_from_chain<R: Rng>(chain0: &Chain, rng: &mut R) -> Vec<ChainSyncMessage> {
    let mut messages = Vec::new();
    let mut msg_id = 0;

    fn walk_chain<R: Rng>(
        chain0: &Chain,
        chain: &Chain,
        messages: &mut Vec<ChainSyncMessage>,
        msg_id: &mut u64,
        rng: &mut R,
        visited: &mut HashMap<String, HashSet<usize>>,
    ) {
        messages.push(ChainSyncMessage::Fwd {
            msg_id: *msg_id,
            slot: Slot::from(chain.block.slot),
            hash: Bytes {
                bytes: chain.block.hash.clone().as_bytes().to_vec(),
            },
            header: Bytes {
                bytes: chain.block.header.clone().as_bytes().to_vec(),
            },
        });
        *msg_id += 1;
        if chain.children.is_empty() {
            // Backtrack.
            let mut ancestors: Vec<Block> = find_ancestors(&chain0.clone(), &chain.block.hash);
            ancestors.reverse();
            for ancestor in &ancestors {
                // XXX: Only go back to ancestors that have unvisited children.
                messages.push(ChainSyncMessage::Bck {
                    msg_id: *msg_id,
                    slot: Slot::from(ancestor.slot),
                    hash: Bytes {
                        bytes: ancestor.hash.clone().as_bytes().to_vec(),
                    },
                });
                *msg_id += 1;
                let ancestor_chain = find_chain(&chain0.clone(), &ancestor.hash)
                    .expect("ancestor has to be in the original chain");
                walk_chain(chain0, &ancestor_chain, messages, msg_id, rng, visited)
            }
        } else {
            let already_visited: HashSet<usize> = visited
                .get(&chain.block.hash)
                .unwrap_or(&HashSet::new())
                .clone();

            let not_visited: Vec<usize> = (0..chain.children.len())
                .collect::<HashSet<usize>>()
                .difference(&already_visited)
                .cloned()
                .collect();

            if not_visited.is_empty() {
                // Backtrack.
                let mut ancestors: Vec<Block> = find_ancestors(&chain0.clone(), &chain.block.hash);
                ancestors.reverse();
                for ancestor in &ancestors {
                    // XXX: Only go back to ancestors that have unvisited children.
                    messages.push(ChainSyncMessage::Bck {
                        msg_id: *msg_id,
                        slot: Slot::from(ancestor.slot),
                        hash: Bytes {
                            bytes: ancestor.hash.clone().as_bytes().to_vec(),
                        },
                    });
                    *msg_id += 1;
                    let ancestor_chain = find_chain(&chain0.clone(), &ancestor.hash)
                        .expect("ancestor has to be in the original chain");
                    walk_chain(chain0, &ancestor_chain, messages, msg_id, rng, visited)
                }
            } else {
                let random_not_visited_index = rng.random_range(0..not_visited.len());
                let index = not_visited[random_not_visited_index];
                visited
                    .entry(chain.block.hash.clone())
                    .and_modify(|visited_indices| {
                        let _bool = visited_indices.insert(index);
                    });
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
    }

    fn find_chain(chain: &Chain, hash: &String) -> Option<Chain> {
        if chain.block.hash == *hash {
            Some(chain.clone())
        } else {
            for child in &chain.children {
                if child.block.hash == *hash {
                    return Some(child.clone());
                } else {
                    match find_chain(&child, hash) {
                        None => continue,
                        Some(child_chain) => return Some(child_chain),
                    }
                }
            }
            None
        }
    }

    walk_chain(
        &chain0,
        &chain0,
        &mut messages,
        &mut msg_id,
        rng,
        &mut HashMap::new(),
    );
    messages
}

pub fn generate_inputs<R: Rng>(rng: &mut R) -> Vec<ChainSyncMessage> {
    let data = read_chain_json();
    match parse_json(data.as_bytes()) {
        Ok(blocks) => {
            let chain = recreate_chain(blocks);
            generate_inputs_from_chain(&chain, rng)
        }
        Err(err) => panic!("{}", err),
    }
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
        assert!(find_ancestors(&chain, "a") == Vec::<Block>::new());
        assert!(find_ancestors(&chain, "c") == vec![block_a, block_b.clone()]);
        assert!(find_ancestors(&chain, "d") == vec![block_b]);
    }

    #[test]
    fn test_recreate_chain() {
        let data = read_chain_json();
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
                result.push(format!("|"));
                result.extend(shift("`- ", "   ", &draw(chain)));
            } else {
                result.push(format!("|"));
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

    use rand::rngs::StdRng;
    use rand::SeedableRng;

    #[test]
    fn test_generate_inputs() {
        let data = read_chain_json();
        match parse_json(data.as_bytes()) {
            Ok(blocks) => {
                let seed = 123;
                let mut rng = StdRng::seed_from_u64(seed);
                let chain = recreate_chain(blocks);
                let inputs = generate_inputs_from_chain(&chain, &mut rng);
                for input in &inputs {
                    println!("{:?}\n", input);
                }
            }
            Err(e) => eprintln!("Error parsing JSON: {}", e),
        }
    }
}
