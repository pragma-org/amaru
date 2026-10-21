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

use super::value::Value;
use crate::{arena::Arena, binder::Eval};

#[derive(Debug)]
pub enum Env<'a, V>
where
    V: Eval<'a>,
{
    Empty,
    Cons { data: &'a Value<'a, V>, next: &'a Env<'a, V> },
}

impl<'a, V> Env<'a, V>
where
    V: Eval<'a>,
{
    pub fn new_in(arena: &'a Arena) -> &'a Self {
        arena.alloc(Self::Empty)
    }

    pub fn push(&'a self, arena: &'a Arena, arg: &'a Value<'a, V>) -> &'a Self {
        arena.alloc(Self::Cons { data: arg, next: self })
    }

    // De Bruijn indices are 1-based
    // So the data at the env[i] is at De Bruijn index i-1
    pub fn lookup(&self, index: usize) -> Option<&'a Value<'a, V>> {
        if index == 0 {
            return None;
        }

        let mut env = self;
        let mut index = index;

        loop {
            match env {
                Env::Empty => return None,
                Env::Cons { data, next } => {
                    if index == 1 {
                        return Some(data);
                    }

                    index -= 1;
                    env = next;
                }
            }
        }
    }
}
