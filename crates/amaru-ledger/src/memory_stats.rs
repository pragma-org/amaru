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

#[cfg(all(feature = "jemalloc", not(target_family = "windows")))]
use std::ffi::{CStr, c_void};
use std::{
    env,
    fs::File,
    io::{BufWriter, Write},
    path::PathBuf,
    sync::{Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

use amaru_kernel::Epoch;

use crate::store::StoreMemoryStats;

const MEMORY_STATS_FILE: &str = "AMARU_MEMORY_STATS_FILE";

/// Emit a profiling sample for the current process and the storage backend.
///
/// Samples are appended to `AMARU_MEMORY_STATS_FILE` when that environment variable is set.
/// The function is intentionally best-effort: failed instrumentation never affects ledger work.
pub fn record(stage: &'static str, epoch: Epoch, store: Option<StoreMemoryStats>) {
    let Some(writer) = writer() else {
        return;
    };

    let timestamp_ms =
        SystemTime::now().duration_since(UNIX_EPOCH).map(|duration| duration.as_millis() as u64).unwrap_or_default();
    let allocator = allocator_stats();
    let store = store.unwrap_or_default();

    let Ok(mut writer) = writer.lock() else {
        return;
    };

    let _ = writeln!(
        writer,
        "{timestamp_ms},{stage},{epoch},{jemalloc_allocated},{jemalloc_active},{jemalloc_metadata},{jemalloc_resident},{jemalloc_mapped},{jemalloc_retained},{block_cache_capacity},{block_cache_usage},{block_cache_pinned_usage},{cur_size_all_mem_tables},{size_all_mem_tables},{estimate_table_readers_mem}",
        jemalloc_allocated = allocator.allocated,
        jemalloc_active = allocator.active,
        jemalloc_metadata = allocator.metadata,
        jemalloc_resident = allocator.resident,
        jemalloc_mapped = allocator.mapped,
        jemalloc_retained = allocator.retained,
        block_cache_capacity = store.block_cache_capacity,
        block_cache_usage = store.block_cache_usage,
        block_cache_pinned_usage = store.block_cache_pinned_usage,
        cur_size_all_mem_tables = store.cur_size_all_mem_tables,
        size_all_mem_tables = store.size_all_mem_tables,
        estimate_table_readers_mem = store.estimate_table_readers_mem,
    );
    let _ = writer.flush();
}

fn writer() -> Option<&'static Mutex<BufWriter<File>>> {
    static WRITER: OnceLock<Option<Mutex<BufWriter<File>>>> = OnceLock::new();

    WRITER
        .get_or_init(|| {
            let path = PathBuf::from(env::var_os(MEMORY_STATS_FILE)?);
            let mut writer = BufWriter::new(File::create(path).ok()?);
            writeln!(
                writer,
                "timestamp_ms,stage,epoch,jemalloc_allocated,jemalloc_active,jemalloc_metadata,jemalloc_resident,jemalloc_mapped,jemalloc_retained,block_cache_capacity,block_cache_usage,block_cache_pinned_usage,cur_size_all_mem_tables,size_all_mem_tables,estimate_table_readers_mem"
            )
            .ok()?;
            writer.flush().ok()?;
            Some(Mutex::new(writer))
        })
        .as_ref()
}

#[derive(Debug, Default, Clone, Copy)]
struct AllocatorStats {
    allocated: u64,
    active: u64,
    metadata: u64,
    resident: u64,
    mapped: u64,
    retained: u64,
}

#[cfg(all(feature = "jemalloc", not(target_family = "windows")))]
fn allocator_stats() -> AllocatorStats {
    refresh_jemalloc_epoch();

    AllocatorStats {
        allocated: read_jemalloc_stat(STATS_ALLOCATED),
        active: read_jemalloc_stat(STATS_ACTIVE),
        metadata: read_jemalloc_stat(STATS_METADATA),
        resident: read_jemalloc_stat(STATS_RESIDENT),
        mapped: read_jemalloc_stat(STATS_MAPPED),
        retained: read_jemalloc_stat(STATS_RETAINED),
    }
}

#[cfg(not(all(feature = "jemalloc", not(target_family = "windows"))))]
fn allocator_stats() -> AllocatorStats {
    AllocatorStats::default()
}

#[cfg(all(feature = "jemalloc", not(target_family = "windows")))]
const JEMALLOC_EPOCH: &CStr = unsafe { CStr::from_bytes_with_nul_unchecked(b"epoch\0") };
#[cfg(all(feature = "jemalloc", not(target_family = "windows")))]
const STATS_ALLOCATED: &CStr = unsafe { CStr::from_bytes_with_nul_unchecked(b"stats.allocated\0") };
#[cfg(all(feature = "jemalloc", not(target_family = "windows")))]
const STATS_ACTIVE: &CStr = unsafe { CStr::from_bytes_with_nul_unchecked(b"stats.active\0") };
#[cfg(all(feature = "jemalloc", not(target_family = "windows")))]
const STATS_METADATA: &CStr = unsafe { CStr::from_bytes_with_nul_unchecked(b"stats.metadata\0") };
#[cfg(all(feature = "jemalloc", not(target_family = "windows")))]
const STATS_RESIDENT: &CStr = unsafe { CStr::from_bytes_with_nul_unchecked(b"stats.resident\0") };
#[cfg(all(feature = "jemalloc", not(target_family = "windows")))]
const STATS_MAPPED: &CStr = unsafe { CStr::from_bytes_with_nul_unchecked(b"stats.mapped\0") };
#[cfg(all(feature = "jemalloc", not(target_family = "windows")))]
const STATS_RETAINED: &CStr = unsafe { CStr::from_bytes_with_nul_unchecked(b"stats.retained\0") };

#[cfg(all(feature = "jemalloc", not(target_family = "windows")))]
fn refresh_jemalloc_epoch() {
    let mut epoch = 1u64;

    unsafe {
        let _ = tikv_jemalloc_sys::mallctl(
            JEMALLOC_EPOCH.as_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            (&mut epoch as *mut u64).cast::<c_void>(),
            std::mem::size_of_val(&epoch),
        );
    }
}

#[cfg(all(feature = "jemalloc", not(target_family = "windows")))]
fn read_jemalloc_stat(name: &CStr) -> u64 {
    let mut value = 0usize;
    let mut size = std::mem::size_of::<usize>();

    let rc = unsafe {
        tikv_jemalloc_sys::mallctl(
            name.as_ptr(),
            (&mut value as *mut usize).cast::<c_void>(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };

    if rc == 0 { value as u64 } else { 0 }
}
