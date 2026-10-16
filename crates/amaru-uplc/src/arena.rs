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

use std::any::type_name;

use append_only_vec::AppendOnlyVec;
use bumpalo::Bump;

use crate::constant::Integer;

pub struct Arena {
    bump: Bump,
    integers: AppendOnlyVec<Integer>,
}

impl Arena {
    pub fn new() -> Self {
        Self { bump: Bump::new(), integers: AppendOnlyVec::new() }
    }

    pub fn from_bump(bump: Bump) -> Self {
        Self { bump, integers: AppendOnlyVec::new() }
    }

    pub fn alloc<T>(&self, value: T) -> &mut T {
        if cfg!(debug_assertions) {
            assert!(type_name::<T>() != type_name::<Integer>(), "use alloc_integer for Integer types");
        }
        self.bump.alloc(value)
    }

    pub fn alloc_integer(&self, value: Integer) -> &Integer {
        let idx = self.integers.push(value);
        &self.integers[idx]
    }

    pub(crate) fn as_bump(&self) -> &Bump {
        &self.bump
    }

    pub fn allocated_bytes(&self) -> usize {
        self.bump.allocated_bytes()
    }

    /// Returns the bytes occupied by allocations in the arena's chunks.
    ///
    /// Unlike [`Self::allocated_bytes`], this excludes unused capacity in the
    /// chunks. This is useful for measuring the live high-water mark of an
    /// evaluation when the arena has been given an initial capacity.
    pub fn used_bytes(&self) -> usize {
        // We only inspect chunk lengths; the raw iterator's pointers are never
        // dereferenced, so the iterator's initialization guarantees are not
        // relevant here.
        unsafe { self.bump.iter_allocated_chunks_raw().map(|(_, length)| length).sum() }
    }

    pub fn reset(&mut self) {
        // Drop all allocated integers
        self.integers = AppendOnlyVec::new();
        self.bump.reset();
    }
}

impl Default for Arena {
    fn default() -> Self {
        Self::new()
    }
}
