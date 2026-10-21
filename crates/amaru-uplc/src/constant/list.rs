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

use std::fmt;

use super::Constant;
use crate::arena::Arena;

#[derive(Clone, Copy)]
pub struct List<'a> {
    representation: ListRepresentation<'a>,
    len: usize,
}

#[derive(Clone, Copy)]
enum ListRepresentation<'a> {
    Flat(&'a [&'a Constant<'a>]),
    Cons { head: &'a Constant<'a>, tail: &'a List<'a> },
}

impl fmt::Debug for List<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_list().entries(self.iter()).finish()
    }
}

impl PartialEq for List<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.len == other.len && self.iter().eq(other.iter())
    }
}

impl<'a> List<'a> {
    pub(super) fn flat(values: &'a [&'a Constant<'a>]) -> Self {
        Self { representation: ListRepresentation::Flat(values), len: values.len() }
    }

    pub(super) fn cons(head: &'a Constant<'a>, tail: &'a Self) -> Self {
        Self { representation: ListRepresentation::Cons { head, tail }, len: tail.len + 1 }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn first(&self) -> Option<&'a Constant<'a>> {
        match self.representation {
            ListRepresentation::Flat(values) => values.first().copied(),
            ListRepresentation::Cons { head, .. } => Some(head),
        }
    }

    pub fn get(&self, index: usize) -> Option<&'a Constant<'a>> {
        if index >= self.len {
            return None;
        }

        let mut current = self;
        let mut index = index;

        loop {
            match current.representation {
                ListRepresentation::Flat(values) => return values.get(index).copied(),
                ListRepresentation::Cons { head, tail } => {
                    if index == 0 {
                        return Some(head);
                    }

                    index -= 1;
                    current = tail;
                }
            }
        }
    }

    pub fn tail(&'a self, arena: &'a Arena) -> Option<&'a Self> {
        match self.representation {
            ListRepresentation::Flat(values) => values.get(1..).map(|tail| &*arena.alloc(Self::flat(tail))),
            ListRepresentation::Cons { tail, .. } => Some(tail),
        }
    }

    pub fn drop(&'a self, arena: &'a Arena, count: usize) -> &'a Self {
        let mut current = self;
        let mut count = count;

        while count > 0 {
            match current.representation {
                ListRepresentation::Flat(values) => {
                    let offset = count.min(values.len());
                    return arena.alloc(Self::flat(&values[offset..]));
                }
                ListRepresentation::Cons { tail, .. } => {
                    current = tail;
                    count -= 1;
                }
            }
        }

        current
    }

    pub fn iter(&'a self) -> ListIter<'a> {
        ListIter { current: Some(self), flat: None, remaining: self.len }
    }
}

pub struct ListIter<'a> {
    current: Option<&'a List<'a>>,
    flat: Option<std::slice::Iter<'a, &'a Constant<'a>>>,
    remaining: usize,
}

impl<'a> Iterator for ListIter<'a> {
    type Item = &'a Constant<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(values) = self.flat.as_mut() {
                if let Some(value) = values.next() {
                    self.remaining -= 1;
                    return Some(*value);
                }

                self.flat = None;
            }

            let current = self.current.take()?;
            match current.representation {
                ListRepresentation::Flat(values) => self.flat = Some(values.iter()),
                ListRepresentation::Cons { head, tail } => {
                    self.current = Some(tail);
                    self.remaining -= 1;
                    return Some(head);
                }
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl ExactSizeIterator for ListIter<'_> {}

impl<'a> IntoIterator for &'a List<'a> {
    type Item = &'a Constant<'a>;
    type IntoIter = ListIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}
