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

use std::{
    alloc::{GlobalAlloc, Layout},
    sync::atomic::{AtomicUsize, Ordering},
};

use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, get_current_pid};

#[derive(Debug, Clone, Copy)]
pub struct AllocationSnapshot {
    pub current_allocated_bytes: usize,
    pub peak_allocated_bytes: usize,
}

pub struct CountingAllocator<A> {
    inner: A,
    current_allocated_bytes: AtomicUsize,
    peak_allocated_bytes: AtomicUsize,
}

impl<A> CountingAllocator<A> {
    pub const fn new(inner: A) -> Self {
        Self { inner, current_allocated_bytes: AtomicUsize::new(0), peak_allocated_bytes: AtomicUsize::new(0) }
    }

    pub fn current_allocated_bytes(&self) -> usize {
        self.current_allocated_bytes.load(Ordering::Relaxed)
    }

    pub fn peak_allocated_bytes(&self) -> usize {
        self.peak_allocated_bytes.load(Ordering::Relaxed)
    }

    pub fn snapshot(&self) -> AllocationSnapshot {
        AllocationSnapshot {
            current_allocated_bytes: self.current_allocated_bytes(),
            peak_allocated_bytes: self.peak_allocated_bytes(),
        }
    }

    fn record_allocated_bytes(&self, allocated_bytes: usize) {
        let current = self.current_allocated_bytes.fetch_add(allocated_bytes, Ordering::Relaxed) + allocated_bytes;
        let mut peak = self.peak_allocated_bytes.load(Ordering::Relaxed);
        while current > peak {
            match self.peak_allocated_bytes.compare_exchange_weak(peak, current, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => break,
                Err(observed) => peak = observed,
            }
        }
    }

    fn record_deallocated_bytes(&self, deallocated_bytes: usize) {
        self.current_allocated_bytes.fetch_sub(deallocated_bytes, Ordering::Relaxed);
    }
}

unsafe impl<A: GlobalAlloc> GlobalAlloc for CountingAllocator<A> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { self.inner.alloc(layout) };
        if !ptr.is_null() {
            self.record_allocated_bytes(layout.size());
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { self.inner.alloc_zeroed(layout) };
        if !ptr.is_null() {
            self.record_allocated_bytes(layout.size());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        self.record_deallocated_bytes(layout.size());
        unsafe { self.inner.dealloc(ptr, layout) };
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_ptr = unsafe { self.inner.realloc(ptr, layout, new_size) };
        if !new_ptr.is_null() {
            match new_size.cmp(&layout.size()) {
                std::cmp::Ordering::Greater => self.record_allocated_bytes(new_size - layout.size()),
                std::cmp::Ordering::Less => self.record_deallocated_bytes(layout.size() - new_size),
                std::cmp::Ordering::Equal => {}
            }
        }
        new_ptr
    }
}

#[expect(clippy::panic, reason = "non-production code")]
pub fn current_process_memory() -> f64 {
    let pid = get_current_pid().unwrap_or_else(|e| panic!("unable to get current pid for memory measurement: {e}"));
    let mut system = System::new();
    system.refresh_processes_specifics(ProcessesToUpdate::Some(&[pid]), false, ProcessRefreshKind::everything());
    system
        .process(pid)
        .map(|process| process.memory() as f64)
        .unwrap_or_else(|| panic!("unable to read process memory for pid {pid:?}"))
}

pub fn rss_delta<A>(task: impl FnOnce() -> A) -> (A, i64) {
    let rss_before = current_process_memory();
    let result = task();
    let rss_after = current_process_memory();
    (result, ((rss_after - rss_before) / (1024.0 * 1024.0)).round() as i64)
}
