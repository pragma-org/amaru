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

use crate::simulator::bytes::Bytes;
use serde::Deserialize;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::{fmt, fs};

/// A chain represents a tree of blocks read from a JSON file.
/// Each block may have multiple children, representing forks in the chain.
#[derive(Debug, Clone)]
pub struct Chain {
    pub block: Block,
    pub children: Vec<Chain>,
}

impl Chain {
    /// Load a chain from a JSON file.
    pub fn from_file(file_path: &PathBuf) -> serde_json::error::Result<Self> {
        Ok(Self::from_blocks(Self::read_blocks_from_file(file_path)?))
    }

    /// Load a chain as a flat list of blocks from a JSON file.
    pub(crate) fn read_blocks_from_file(
        file_path: &PathBuf,
    ) -> serde_json::error::Result<Vec<Block>> {
        let json = fs::read_to_string(file_path).unwrap_or_else(|_| panic!("cannot find blocktree file '{}', use --block-tree-file <FILE> to set the file to load block tree from", file_path.display()));
        let blocks: Root = serde_json::from_slice(json.as_bytes())?;
        Ok(blocks.stake_pools.chains)
    }

    /// Recreate a tree of blocks from a flat list of blocks by following the parent hashes.
    pub fn from_blocks(blocks: Vec<Block>) -> Self {
        match blocks.as_slice() {
            [] => panic!("recreate_tree: no blocks"),
            [head, tail @ ..] => {
                assert!(head.parent.is_none(), "first block has a parent");
                let children = Self::recreate_children(&head.hash, tail);
                Chain {
                    block: head.clone(),
                    children,
                }
            }
        }
    }

    /// Draw the chain as a tree.
    pub fn draw(&self) -> String {
        let mut result = vec![format!("{}", self.block)];
        result.extend(self.draw_subchains());
        result.join("\n")
    }

    /// Draw the subchains recursively.
    fn draw_subchains(&self) -> Vec<String> {
        if self.children.is_empty() {
            return vec![];
        }

        // Helper function to shift lines with appropriate prefixes
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

        let mut result = Vec::new();
        for (i, chain) in self.children.iter().enumerate() {
            if i == 0 {
                result.push("|".to_string());
                result.extend(shift("`- ", "   ", &chain.draw_subchains()));
            } else {
                result.push("|".to_string());
                result.extend(shift("+- ", "|  ", &chain.draw_subchains()));
            }
        }
        result
    }

    /// Find the blocks that are ancestors of the block with the given hash.
    pub fn find_ancestors(&self, target_hash: &Bytes) -> Vec<Block> {
        let mut stack = vec![(self, Vec::new())];

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

    /// Find the block with the given hash in the chain
    pub fn find(&self, hash: &Bytes) -> Option<Chain> {
        let mut stack = vec![self];

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

    /// Recreate children chains for a given parent hash from a list of blocks.
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
                let children = Self::recreate_children(&block.hash, &remaining_blocks);
                Chain { block, children }
            })
            .collect()
    }
}

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
    pub(crate) header: Bytes,
    pub height: u32,
    pub(crate) parent: Option<Bytes>,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

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
        assert_eq!(chain.find_ancestors(&a), Vec::<Block>::new());
        assert_eq!(
            chain.find_ancestors(&c),
            vec![block_a.clone(), block_b.clone()]
        );
        assert_eq!(chain.find_ancestors(&d), vec![block_a]);
    }

    #[test]
    fn test_recreate_chain_from_blocks() {
        match Chain::read_blocks_from_file(&Path::new("tests/data/chain.json").into()) {
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
            }
            Err(e) => eprintln!("Error parsing JSON: {}", e),
        }
    }

    #[test]
    fn test_recreate_chain() {
        assert!(Chain::from_file(&Path::new("tests/data/chain.json").into()).is_ok());
    }
}
