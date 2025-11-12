use bumpalo::Bump;
use std::collections::VecDeque;
use std::mem::ManuallyDrop;
use std::sync::{Arc, Condvar, Mutex};

pub type BumpPool = Mutex<VecDeque<Bump>>;

/// A bounded pool of Bumpalo arenas
///
/// When all arenas are in use, `acquire()` will block until one becomes available.
/// This ensures memory usage is bounded to `max_size * arena_size`.
///
/// Arenas will be allocated with `initial_capacity`, but can grow if needed
///
/// The pool can be cheaply cloned and shared across threads.
#[derive(Clone)]
pub struct ArenaPool {
    inner: Arc<Inner>,
}

struct Inner {
    arenas: BumpPool,
    condvar: Condvar,
    max_size: usize,
    initial_capacity: usize,
    total_created: Mutex<usize>,
}

impl ArenaPool {
    /// Create a new arena pool with a maximum size.
    ///
    ///
    /// At most `max_size` arenas will exist simultaneously.
    /// If all arenas are in use, `acquire()` will block.
    pub fn new(max_size: usize, initial_capacity: usize) -> Self {
        Self {
            inner: Arc::new(Inner {
                arenas: Mutex::new(VecDeque::with_capacity(max_size)),
                condvar: Condvar::new(),
                max_size,
                initial_capacity,
                total_created: Mutex::new(0),
            }),
        }
    }

    /// Acquire an arena from the pool
    ///
    /// Blocks if all arenas are in use, waiting for one to be returned.
    #[allow(unused_assignments)]
    pub fn acquire(&self) -> PooledArena {
        let arena = loop {
            // FIXME: We may need to not ignore a poisoned mutex here
            let mut guard = self.inner.arenas.lock().unwrap_or_else(|p| p.into_inner());

            if let Some(arena) = guard.pop_front() {
                break arena;
            }

            // FIXME: We may need to not ignore a poisoned mutex here
            let mut total = self
                .inner
                .total_created
                .lock()
                .unwrap_or_else(|p| p.into_inner());

            if *total < self.inner.max_size {
                *total += 1;
                drop(total);
                drop(guard);
                break self.new_arena();
            }

            drop(total);
            // Condvar blocks the current thread until we're notified of a free arena
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
        if let Some(arena) = guard.pop_front() {
            return Some(PooledArena {
                arena: ManuallyDrop::new(arena),
                pool: self.inner.clone(),
            });
        }

        let mut total = self
            .inner
            .total_created
            .lock()
            .unwrap_or_else(|p| p.into_inner());

        if *total < self.inner.max_size {
            *total += 1;
            drop(total);
            drop(guard);
            return Some(PooledArena {
                arena: ManuallyDrop::new(self.new_arena()),
                pool: self.inner.clone(),
            });
        }

        None
    }

    /// Current number of available arenas in the pool
    pub fn available(&self) -> usize {
        self.inner
            .arenas
            .lock()
            .map(|guard| guard.len())
            .unwrap_or(0)
    }

    /// Total number of arenas created
    pub fn total_arenas(&self) -> usize {
        self.inner
            .total_created
            .lock()
            .map(|guard| *guard)
            .unwrap_or(0)
    }

    fn new_arena(&self) -> Bump {
        Bump::with_capacity(self.inner.initial_capacity)
    }
}

/// RAII Guard for the PooledArena (https://rust-unofficial.github.io/patterns/patterns/behavioural/RAII.html)
///
/// Returns arenas to the pool when dropped
pub struct PooledArena {
    arena: ManuallyDrop<Bump>,
    pool: Arc<Inner>,
}

impl AsRef<Bump> for PooledArena {
    fn as_ref(&self) -> &Bump {
        &self.arena
    }
}

impl Drop for PooledArena {
    fn drop(&mut self) {
        // SAFETY NOTE: It's important we only take the arena once, here in the drop implementation
        let mut arena = unsafe { ManuallyDrop::take(&mut self.arena) };

        arena.reset();

        // We're ignoring a poisoned mutex here
        let mut pool = self.pool.arenas.lock().unwrap_or_else(|p| p.into_inner());
        pool.push_back(arena);
        self.pool.condvar.notify_one();
    }
}

impl std::ops::Deref for PooledArena {
    type Target = Bump;

    fn deref(&self) -> &Self::Target {
        self.as_ref()
    }
}
