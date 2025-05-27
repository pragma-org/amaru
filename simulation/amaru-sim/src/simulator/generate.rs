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

use serde::Deserialize;
use serde_json::Result;
use std::fmt;
use std::fs;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Root {
    stake_pools: StakePools,
}

#[derive(Debug, Deserialize)]
struct StakePools {
    chains: Vec<Block>,
}

#[derive(Clone, Debug, Deserialize)]
#[allow(unused)]
struct Block {
    hash: String,
    header: String,
    height: u32,
    parent: Option<String>,
    slot: u32,
}

impl fmt::Display for Block {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "<hash: {}, height: {}>", &self.hash[..6], self.height)
    }
}

#[allow(unused)]
fn read_chain_json() -> String {
    let file_path = "tests/data/chain.json";
    fs::read_to_string(file_path).expect("Should have been able to read the chain.json file")
}

#[allow(unused)]
fn parse_json(bytes: &[u8]) -> Result<Vec<Block>> {
    let result: Root = serde_json::from_slice(bytes)?;
    Ok(result.stake_pools.chains)
}

#[derive(Debug)]
struct Chain {
    block: Block,
    children: Vec<Chain>,
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

fn draw_chain(chain: &Chain) -> String {
    let lines = draw(chain);
    lines.join("\n")
}

fn draw_children(children: &[Chain]) -> String {
    children
        .iter()
        .map(draw_chain)
        .collect::<Vec<_>>()
        .join("\n")
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

#[cfg(test)]
mod test {
    use crate::simulator::generate::*;

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
}
