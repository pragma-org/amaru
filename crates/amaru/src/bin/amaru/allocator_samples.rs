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
    env,
    fs::File,
    io::{self, BufWriter, Write},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use amaru_kernel::utils::memory::AllocationSnapshot;

const ALLOCATOR_SAMPLES_FILE: &str = "AMARU_ALLOCATOR_SAMPLES_FILE";
const ALLOCATOR_SAMPLES_INTERVAL_MS: &str = "AMARU_ALLOCATOR_SAMPLES_INTERVAL_MS";
const DEFAULT_INTERVAL_MS: u64 = 1_000;

/// Periodically records live and peak heap bytes when the counting allocator is enabled.
///
/// Sampling is opt-in and controlled entirely by environment variables so production runs do not
/// pay the background-thread overhead unless explicitly requested.
pub struct Sampler {
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<io::Result<()>>>,
}

impl Sampler {
    /// Start a heap sampler when `AMARU_ALLOCATOR_SAMPLES_FILE` is set.
    pub fn spawn_from_env(sample: fn() -> AllocationSnapshot) -> io::Result<Option<Self>> {
        let Some(path) = env::var_os(ALLOCATOR_SAMPLES_FILE).map(PathBuf::from) else {
            return Ok(None);
        };

        let interval = sampling_interval();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker = thread::Builder::new()
            .name("amaru-allocator-sampler".into())
            .spawn(move || run_sampler(path, interval, worker_stop, sample))?;

        Ok(Some(Self { stop, worker: Some(worker) }))
    }

    /// Stop the sampler and flush the output file.
    pub fn shutdown(mut self) -> io::Result<()> {
        self.stop.store(true, Ordering::Relaxed);

        match self.worker.take().expect("sampler worker is always present").join() {
            Ok(result) => result,
            Err(_) => Err(io::Error::other("allocator sampler thread panicked")),
        }
    }
}

fn run_sampler(
    path: PathBuf,
    interval: Duration,
    stop: Arc<AtomicBool>,
    sample: fn() -> AllocationSnapshot,
) -> io::Result<()> {
    let mut writer = BufWriter::new(File::create(path)?);
    writeln!(writer, "timestamp_ms,heap_live_bytes,heap_peak_bytes")?;

    while !stop.load(Ordering::Relaxed) {
        let AllocationSnapshot { current_allocated_bytes, peak_allocated_bytes } = sample();
        writeln!(writer, "{},{},{}", timestamp_ms(), current_allocated_bytes, peak_allocated_bytes)?;
        writer.flush()?;
        thread::sleep(interval);
    }

    Ok(())
}

fn sampling_interval() -> Duration {
    let interval_ms = env::var(ALLOCATOR_SAMPLES_INTERVAL_MS)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_INTERVAL_MS);

    Duration::from_millis(interval_ms)
}

fn timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}
