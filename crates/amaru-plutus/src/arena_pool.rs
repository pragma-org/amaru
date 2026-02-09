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

use std::{
    collections::VecDeque,
    mem::ManuallyDrop,
    sync::{Arc, Condvar, Mutex},
};

use uplc_turbo::{arena::Arena, bumpalo::Bump};

pub type BumpPool = Mutex<VecDeque<Arena>>;

/// A bounded pool of Bumpalo arenas
///
/// All arenas are pre-allocated at creation time.
/// When all arenas are in use, `acquire()` will block until one becomes available.
///
/// The pool can be cheaply cloned for use across threads
#[derive(Clone)]
pub struct ArenaPool {
    inner: Arc<Inner>,
}

struct Inner {
    arenas: BumpPool,
    condvar: Condvar,
}

impl ArenaPool {
    /// Create a new arena pool with a fixed number of pre-allocated arenas.
    ///
    /// All `size` arenas are created immediately with `initial_capacity` bytes each.
    /// If all arenas are in use, `acquire()` will block.
    pub fn new(size: usize, initial_capacity: usize) -> Self {
        let mut arenas = VecDeque::with_capacity(size);
        for _ in 0..size {
            arenas.push_back(Arena::from_bump(Bump::with_capacity(initial_capacity)));
        }

        Self {
            inner: Arc::new(Inner {
                arenas: Mutex::new(arenas),
                condvar: Condvar::new(),
            }),
        }
    }

    /// Acquire an arena from the pool
    ///
    /// Blocks if all arenas are in use, waiting for one to be returned.
    pub fn acquire(&self) -> PooledArena {
        let arena = loop {
            let mut guard = self.inner.arenas.lock().unwrap_or_else(|p| p.into_inner());

            if let Some(arena) = guard.pop_front() {
                break arena;
            }

            guard = self
                .inner
                .condvar
                .wait(guard)
                .unwrap_or_else(|p| p.into_inner());
        };

        PooledArena {
            arena: ManuallyDrop::new(arena),
            pool: self.inner.clone(),
        }
    }

    /// Try to acquire an arena from the pool
    ///
    /// Non-blocking, returns None if all arenas are in use
    pub fn try_acquire(&self) -> Option<PooledArena> {
        let mut guard = self.inner.arenas.lock().unwrap_or_else(|p| p.into_inner());

        guard.pop_front().map(|arena| PooledArena {
            arena: ManuallyDrop::new(arena),
            pool: self.inner.clone(),
        })
    }
}

/// RAII Guard for the PooledArena (https://rust-unofficial.github.io/patterns/patterns/behavioural/RAII.html)
///
/// Returns arenas to the pool when dropped
pub struct PooledArena {
    arena: ManuallyDrop<Arena>,
    pool: Arc<Inner>,
}

impl AsRef<Arena> for PooledArena {
    fn as_ref(&self) -> &Arena {
        &self.arena
    }
}

impl Drop for PooledArena {
    fn drop(&mut self) {
        // SAFETY NOTE: It's important we only take the arena once, here in the drop implementation
        let mut arena = unsafe { ManuallyDrop::take(&mut self.arena) };
        arena.reset();

        let mut pool = self.pool.arenas.lock().unwrap_or_else(|p| p.into_inner());
        pool.push_back(arena);
        self.pool.condvar.notify_one();
    }
}

impl std::ops::Deref for PooledArena {
    type Target = Arena;

    fn deref(&self) -> &Self::Target {
        self.as_ref()
    }
}
