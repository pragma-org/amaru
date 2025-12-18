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

use crate::{HasIndex, Redeemer};
use std::{borrow::Cow, cmp::Ordering, ops::Deref};

/// A type that provides Ord and PartialOrd instance on redeemers, to allow storing them in binary
/// trees in a controlled order (that matches Haskell's implementation).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrderedRedeemer<'a>(Cow<'a, Redeemer>);

impl Ord for OrderedRedeemer<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        match self.tag.as_index().cmp(&other.tag.as_index()) {
            by_tag @ Ordering::Less | by_tag @ Ordering::Greater => by_tag,
            Ordering::Equal => self.index.cmp(&other.index),
        }
    }
}

impl PartialOrd for OrderedRedeemer<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Deref for OrderedRedeemer<'_> {
    type Target = Redeemer;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'a> From<Cow<'a, Redeemer>> for OrderedRedeemer<'a> {
    fn from(value: Cow<'a, Redeemer>) -> Self {
        Self(value)
    }
}

impl From<Redeemer> for OrderedRedeemer<'static> {
    fn from(value: Redeemer) -> Self {
        Self(Cow::Owned(value))
    }
}

impl<'a> From<&'a Redeemer> for OrderedRedeemer<'a> {
    fn from(value: &'a Redeemer) -> Self {
        Self(Cow::Borrowed(value))
    }
}
