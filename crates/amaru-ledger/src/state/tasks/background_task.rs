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
    fmt, mem,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
};

use amaru_kernel::Epoch;

use crate::state::StateError;

/// A background task can be spawned in the background to allow computations to execute while
/// the ledger still makes progress with new blocks. This is used for computing rewards and
/// stake distributions:
///
///  - [`Self::start`] runs a `Runnable` in a new thread, for a given epoch.
///  - [`Self::join`] waits for the thread and returns its result `R`.
///  - [`Self::cancel`] stops the thread and discards its result, for example during a rollback,
///    when the computation becomes irrelevant.
///  - [`Self::finished`] gives the result of a thread that ended, before the join.
///
/// After a join or a cancel, the task is not running, and can start again.
///
/// The epoch is used to validate that the results we get are indeed results expected for a given epoch.
pub struct BackgroundTask<R> {
    name: String,
    state: State<R>,
}

#[derive(Default)]
enum State<R> {
    #[default]
    NotStarted,
    Running {
        epoch: Epoch,
        cancellable: Cancellable,
        handle: JoinHandle<Result<R, StateError>>,
    },
    Finished {
        epoch: Epoch,
        result: Result<R, StateError>,
    },
}

impl<R> fmt::Debug for BackgroundTask<R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = match &self.state {
            State::NotStarted => "NotStarted".to_string(),
            State::Running { epoch, .. } => format!("Running({epoch})"),
            State::Finished { epoch, .. } => format!("Finished({epoch})"),
        };
        f.debug_struct("BackgroundTask").field("name", &self.name).field("state", &state).finish()
    }
}

impl<R: Send + 'static> BackgroundTask<R> {
    /// A task that is not started yet.
    pub fn new(name: &str) -> Self {
        Self { name: name.into(), state: State::NotStarted }
    }

    /// Run a `Runnable` in the background, for a given epoch.
    pub fn start(&mut self, epoch: Epoch, task: impl Runnable<R> + Send + 'static) {
        debug_assert!(!self.is_running(), "the computation is already running: {self:?}");
        let cancellable = Cancellable::default();
        let cancellable_clone = cancellable.clone();
        let dispatch = tracing::dispatcher::get_default(|dispatch| dispatch.clone());
        let handle =
            std::thread::spawn(move || tracing::dispatcher::with_default(&dispatch, || task.run(cancellable_clone)));
        self.state = State::Running { epoch, cancellable, handle };
    }

    /// Wait for the running task, if any, and return its result. The `epoch` passed as a parameter
    /// is used to double-check that the task was started in the correct epoch. Without a running
    /// task, this returns `None`.
    pub fn join(&mut self, epoch: Epoch) -> Result<Option<R>, StateError> {
        let (started_epoch, result) = match mem::take(&mut self.state) {
            State::NotStarted => return Ok(None),
            State::Running { epoch, handle, .. } => (epoch, Self::wait(&self.name, handle)),
            State::Finished { epoch, result } => (epoch, result),
        };
        assert_eq!(
            started_epoch, epoch,
            "invariant violation: the {} task was started in epoch {started_epoch}, but the epoch transition ends \
             epoch {epoch}",
            self.name
        );
        result.map(Some)
    }

    /// The result of a task whose thread ended, kept until [`Self::join`] takes it. `None` while
    /// the thread still executes, and without a task.
    pub fn finished(&mut self) -> Option<&mut Result<R, StateError>> {
        if matches!(&self.state, State::Running { handle, .. } if handle.is_finished())
            && let State::Running { epoch, handle, .. } = mem::take(&mut self.state)
        {
            self.state = State::Finished { epoch, result: Self::wait(&self.name, handle) };
        }
        match &mut self.state {
            State::Finished { result, .. } => Some(result),
            State::NotStarted | State::Running { .. } => None,
        }
    }

    fn wait(name: &str, handle: JoinHandle<Result<R, StateError>>) -> Result<R, StateError> {
        handle.join().map_err(|_| StateError::BackgroundTaskFailed { task: name.to_string() })?
    }

    /// Stop the running task, if any, and wait for it. Its result is discarded; a panic in a thread
    /// that this call waits for is a bug and propagates, unless this thread is already unwinding.
    pub fn cancel(&mut self) {
        if let State::Running { cancellable, handle, .. } = mem::take(&mut self.state) {
            cancellable.cancel();
            if let Err(panic) = handle.join()
                && !std::thread::panicking()
            {
                std::panic::resume_unwind(panic);
            }
        }
    }

    /// Whether a task was started, and neither joined nor cancelled yet. Its thread could have ended.
    pub fn is_running(&self) -> bool {
        !matches!(self.state, State::NotStarted)
    }

    /// Whether the thread of the task is still executing.
    pub fn is_executing(&self) -> bool {
        matches!(&self.state, State::Running { handle, .. } if !handle.is_finished())
    }
}

pub trait Runnable<R> {
    fn run(self, cancellable: Cancellable) -> Result<R, StateError>;
}

/// Two computations in one task: the first one runs, then the second one. Both stop when the task
/// is cancelled.
impl<A: Runnable<RA>, B: Runnable<RB>, RA, RB> Runnable<(RA, RB)> for (A, B) {
    fn run(self, cancellable: Cancellable) -> Result<(RA, RB), StateError> {
        let first = self.0.run(cancellable.clone())?;
        let second = self.1.run(cancellable)?;
        Ok((first, second))
    }
}

#[derive(Default, Debug, Clone)]
pub struct Cancellable {
    cancelled: Arc<AtomicBool>,
}

impl Cancellable {
    pub fn check_cancelled(&self) -> Result<(), StateError> {
        if self.cancelled.load(Ordering::Relaxed) { Err(StateError::BackgroundTaskCancelled) } else { Ok(()) }
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{self, Receiver};

    use super::*;

    struct WaitFor(Receiver<u64>);

    impl Runnable<u64> for WaitFor {
        fn run(self, _cancellable: Cancellable) -> Result<u64, StateError> {
            self.0.recv().map_err(|_| StateError::BackgroundTaskCancelled)
        }
    }

    const EPOCH: Epoch = Epoch::new(10);

    #[test]
    fn a_task_has_no_result_while_its_thread_executes() {
        let (_sender, receiver) = mpsc::channel();
        let mut task = BackgroundTask::new("test");
        task.start(EPOCH, WaitFor(receiver));

        assert!(task.is_running() && task.is_executing());
        assert!(task.finished().is_none());
    }

    #[test]
    fn a_finished_task_keeps_its_result_until_the_join() {
        let (sender, receiver) = mpsc::channel();
        let mut task = BackgroundTask::new("test");
        task.start(EPOCH, WaitFor(receiver));
        sender.send(42).unwrap();
        while task.is_executing() {
            std::thread::yield_now();
        }

        assert!(matches!(task.finished(), Some(Ok(42))));
        assert!(task.is_running(), "the task is not joined yet");
        assert!(matches!(task.join(EPOCH), Ok(Some(42))));
        assert!(!task.is_running());
    }

    #[test]
    fn a_finished_task_finished_twice_keeps_its_result() {
        let (sender, receiver) = mpsc::channel();
        let mut task = BackgroundTask::new("test");
        task.start(EPOCH, WaitFor(receiver));
        sender.send(42).unwrap();
        while task.is_executing() {
            std::thread::yield_now();
        }

        assert!(matches!(task.finished(), Some(Ok(42))));
        assert!(matches!(task.finished(), Some(Ok(42))));
        assert!(matches!(task.join(EPOCH), Ok(Some(42))));
        assert!(!task.is_running());
    }
}
