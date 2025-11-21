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

use core::fmt;
use std::{borrow::Borrow, sync::Arc};

/// A mapping from an Arc-owned type to another. The transformation must be fully known at
/// compile-time to allow for the entire structure to be cloned.
///
/// Also, to ease the borrow-checker, it is required that `A` and `B` are also plain types (or
/// statically borrowed references) but cannot themselves hold any lifetime shorter than 'static.
#[derive(Clone)]
pub struct ArcMapped<A: 'static, B: 'static> {
    arc: Arc<A>,
    transform: &'static fn(&A) -> &B,
}

impl<A, B: fmt::Debug> fmt::Debug for ArcMapped<A, B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        let b: &B = self.borrow();
        b.fmt(f)
    }
}

impl<A: 'static, B: 'static> ArcMapped<A, B> {
    pub fn new(arc: Arc<A>, transform: &'static fn(&A) -> &B) -> Self {
        Self { arc, transform }
    }
}

/// Obtain a handle on `B` by simply applying the transformation function on the borrowed Arc<A>
impl<A, B> std::borrow::Borrow<B> for ArcMapped<A, B> {
    fn borrow(&self) -> &B {
        let f = &self.transform;
        f(self.arc.as_ref())
    }
}
