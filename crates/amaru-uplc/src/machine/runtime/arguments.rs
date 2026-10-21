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

use std::ops::Index;

use crate::{arena::Arena, binder::Eval, machine::Value};

/// An arguments list represented as a chain of pointers of already existing values. This avoids
/// allocating a vector of arguments for every builtin application.
#[derive(Debug)]
pub struct Arguments<'a, V>
where
    V: Eval<'a>,
{
    head: Option<&'a Argument<'a, V>>,
    len: usize,
}

#[derive(Debug)]
struct Argument<'a, V>
where
    V: Eval<'a>,
{
    value: &'a Value<'a, V>,
    previous: Option<&'a Argument<'a, V>>,
}

impl<'a, V> Copy for Arguments<'a, V> where V: Eval<'a> {}

impl<'a, V> Clone for Arguments<'a, V>
where
    V: Eval<'a>,
{
    fn clone(&self) -> Self {
        *self
    }
}

impl<'a, V> Arguments<'a, V>
where
    V: Eval<'a>,
{
    pub fn empty() -> Self {
        Self { head: None, len: 0 }
    }

    pub fn push(&self, arena: &'a Arena, value: &'a Value<'a, V>) -> Self {
        let argument = arena.alloc(Argument { value, previous: self.head });

        Self { head: Some(argument), len: self.len + 1 }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn node(&self, index: usize) -> Option<&'a Argument<'a, V>> {
        if index >= self.len {
            return None;
        }

        let mut argument = self.head;

        for _ in 0..self.len - index - 1 {
            argument = argument.and_then(|argument| argument.previous);
        }

        argument
    }

    pub fn get(&self, index: usize) -> Option<&'a Value<'a, V>> {
        self.node(index).map(|argument| argument.value)
    }

    pub fn iter(&self) -> ArgumentsIter<'a, V> {
        ArgumentsIter { arguments: *self, index: 0 }
    }
}

impl<'a, V> Index<usize> for Arguments<'a, V>
where
    V: Eval<'a>,
{
    type Output = &'a Value<'a, V>;

    fn index(&self, index: usize) -> &Self::Output {
        &self.node(index).expect("builtin argument index out of bounds").value
    }
}

pub struct ArgumentsIter<'a, V>
where
    V: Eval<'a>,
{
    arguments: Arguments<'a, V>,
    index: usize,
}

impl<'a, V> Iterator for ArgumentsIter<'a, V>
where
    V: Eval<'a>,
{
    type Item = &'a Value<'a, V>;

    fn next(&mut self) -> Option<Self::Item> {
        let value = self.arguments.get(self.index);
        self.index += 1;
        value
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.arguments.len.saturating_sub(self.index);
        (remaining, Some(remaining))
    }
}

impl<'a, V> ExactSizeIterator for ArgumentsIter<'a, V> where V: Eval<'a> {}

impl<'a, V> IntoIterator for &'_ Arguments<'a, V>
where
    V: Eval<'a>,
{
    type Item = &'a Value<'a, V>;
    type IntoIter = ArgumentsIter<'a, V>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{arena::Arena, binder::DeBruijn, machine::value::Value};

    #[test]
    fn arguments_keep_application_order() {
        let arena = Arena::new();
        let first = Value::<DeBruijn>::integer(&arena, arena.alloc_integer(1.into()));
        let second = Value::<DeBruijn>::integer(&arena, arena.alloc_integer(2.into()));
        let third = Value::<DeBruijn>::integer(&arena, arena.alloc_integer(3.into()));

        let arguments = Arguments::empty().push(&arena, first).push(&arena, second).push(&arena, third);

        assert_eq!(arguments.len(), 3);
        assert!(std::ptr::eq(arguments[0], first));
        assert!(std::ptr::eq(arguments[1], second));
        assert!(std::ptr::eq(arguments[2], third));
        assert_eq!(arguments.iter().count(), 3);
    }
}
