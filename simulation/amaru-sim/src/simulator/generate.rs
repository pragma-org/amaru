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
use std::path::PathBuf;

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
pub struct Block {
    pub hash: Bytes,
    header: Bytes,
    height: u32,
    parent: Option<Bytes>,
    pub slot: u64,
}

impl fmt::Display for Block {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "<hash: {}, slot: {}, height: {}, header: {}>",
            hex::encode(&self.hash.bytes[..6]),
            self.slot,
            self.height,
            hex::encode(&self.header.bytes[..6])
        )
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct Chain {
    block: Block,
    children: Vec<Chain>,
}

pub fn read_chain_json(file_path: &PathBuf) -> String {
    fs::read_to_string(file_path).unwrap_or_else(|_| panic!("cannot find blocktree file '{}', use --block-tree-file <FILE> to set the file to load block tree from", file_path.display()))
}

pub fn parse_json(bytes: &[u8]) -> Result<Vec<Block>> {
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

fn recreate_children(parent_hash: &Bytes, blocks: &[Block]) -> Vec<Chain> {
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

fn find_ancestors(chain: &Chain, target_hash: &Bytes) -> Vec<Block> {
    let mut stack = vec![(chain, Vec::new())];

    while let Some((current_chain, mut ancestors)) = stack.pop() {
        if current_chain.block.hash == *target_hash {
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
                    hash: ancestor.hash.clone(),
                });
                *msg_id += 1;
                walk_chain(chain0, &ancestor_chain, messages, msg_id, rng, visited)
            }
        }
    }

    fn find_chain(chain: &Chain, hash: &Bytes) -> Option<Chain> {
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

pub fn generate_inputs<R: Rng>(rng: &mut R, file_path: &PathBuf) -> Result<Vec<ChainSyncMessage>> {
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
    file_path: &PathBuf,
) -> impl Strategy<Value = Vec<ChainSyncMessage>> + use<'_> {
    any::<u64>().prop_map(|seed| {
        let mut rng = StdRng::seed_from_u64(seed);
        generate_inputs(&mut rng, file_path).unwrap()
    })
}

#[cfg(test)]
mod test {
    use std::path::Path;

    use crate::simulator::generate::*;

    #[test]
    fn test_ancestors() {
        let a = Bytes::try_from("aa").unwrap();
        let b = Bytes::try_from("bb").unwrap();
        let c = Bytes::try_from("cc").unwrap();
        let d = Bytes::try_from("dd").unwrap();
        let dummy = Bytes::try_from("00").unwrap();
        let block_a = Block {
            hash: a.clone(),
            header: dummy.clone(),
            height: 0,
            parent: None,
            slot: 0,
        };

        let block_b = Block {
            hash: b.clone(),
            header: dummy.clone(),
            height: 1,
            parent: Some(a.clone()),
            slot: 1,
        };

        let block_c = Block {
            hash: c.clone(),
            header: dummy.clone(),
            height: 2,
            parent: Some(b.clone()),
            slot: 2,
        };

        let block_d = Block {
            hash: d.clone(),
            header: dummy.clone(),
            height: 1,
            parent: Some(a.clone()),
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
        assert_eq!(find_ancestors(&chain, &a), Vec::<Block>::new());
        assert_eq!(
            find_ancestors(&chain, &c),
            vec![block_a.clone(), block_b.clone()]
        );
        assert_eq!(find_ancestors(&chain, &d), vec![block_a]);
    }

    #[test]
    fn test_recreate_chain() {
        let data = read_chain_json(&Path::new("tests/data/chain.json").into());
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
        let data = read_chain_json(&Path::new("tests/data/chain.json").into());
        match parse_json(data.as_bytes()) {
            Ok(blocks) => {
                assert_eq!(
                    blocks[0].hash,
                    Bytes {
                        bytes: hex::decode(
                            "2487bd4f49c89e59bb3d2166510d3d49017674d3c3b430b95db2e260fedce45e"
                                .as_bytes()
                        )
                        .unwrap()
                    }
                );
                assert_eq!(
                    blocks[0].header,
                    Bytes {
                        bytes: hex::decode(
                                "828a01181ff6582022ff37595005fa65ad731d4fb112de050aa0ca910d9a3110f56f3879d449f88e5820da90997ba81483b3f2b4de751ba3ece6ba6d50f96598eb0940cc7d29452cdcb9825840d350e19abe11a25b28d4d1a846faa8f7792b6dc4a19679780dcdd3d98baf4e9738d82764c59b1c76f80b5ddbff2e145aa26f1652ab83f1f930fc430d1be960305850b444cc452b3fbc01a267ff526ea8a66cb57202303b4b1c22557cad8c1d8afbb47a34e55c5e0bee497f58c812e8b2fed95b40cb1f966ad77445ef573f289910debf5dcb4924cecce47d181f325b4d21040058200a9aa01fbdfe2cff3e49a3c02c2610691966075092f76bd26f5bddf85489ffd3845820499fc5dada1544be26d7cd3b2851fe955b44e560b50901abc71333d5d449eac9182e005840595a9a329b2637b8f2cb501aa793a159acc928a27c01e1ef586508492a68cabeae6ced714421be1f648bb05c7196f38e7aa4a8f616ad46e32c84e67951657a038200005901c0c00b0daf83dfc61a23fa9f6e4db5ad31428a7e98839aba420dbdbdc5ad90b72185cb03b5373b73fcd9c0128b3afcc7d14e5b51f4d5592d55a5d222314b590101472338fcc8b178f0ba10f8683725c5df444b7fc6a5afc3c7fbab82e00e7df2d247c673d066eacc1dc7860c10b134c413d71c5a073a5f3e66a17f0d25dabae2699d7b4a969e129d627cd7839995ddf40a3e6672b6d03936de782f5e0dc31bfafdccc5d5d2e5e9a5b3414bface59a824e3a574250474d633115af821b63232de753fddb638606b93b853144dd75692b02e73b2ef8621eb1bfa307cfda8acaa2c43f8b673c9ed749e472cdf2fced20c063a2507ccb985d2b5a9bc77699f42379fb349e1e3a1ab86d1bd510c6ee89720f860d55c208dd262f49746d8fb2a7817d038262a6b266f98f427fc8b958f11adb8c84cc96444b1f5f9d994a4fd1adae2f2be87d8c1dbc0206ace7871da50cc92476d3fced6a4fc2809c8bb47dff992b06259c21f78cd6e72b49b8fe101065aaf243003af67a11d5aabcebf1059b3c92493f954507573a01bf92047ef68f4e980df914b360307b78b3371138b4c1504e155f704df9fabf1bc87ac47b3d5cd563cee6017380e4df6529face9cf39b36277068e"
                                .as_bytes()
                        )
                        .unwrap()
                    }
                );
                assert_eq!(blocks[0].height, 1);
                assert_eq!(blocks[0].slot, 31);
                assert_eq!(blocks[0].parent, None);
                assert_eq!(
                    blocks[1].parent,
                    Some(Bytes {
                        bytes: hex::decode(
                            "2487bd4f49c89e59bb3d2166510d3d49017674d3c3b430b95db2e260fedce45e"
                                .as_bytes()
                        )
                        .unwrap()
                    })
                );

                let seed = 1234;
                let mut rng = StdRng::seed_from_u64(seed);
                let chain = recreate_chain(blocks);
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
            Err(e) => eprintln!("Error parsing JSON: {}", e),
        }
    }
}
