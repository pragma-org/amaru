use crate::{Name, StageResponse};
use rand::{Rng, SeedableRng, rngs::StdRng};
use std::collections::VecDeque;

/// A strategy for making decisions within the simulation run.
///
/// While it is possible to use `Arc<Mutex<...>>` to create a shared random number generator,
/// this is not what the SimulationBuilder is meant for: if you need to share a random number
/// generator across threads, then the result will not be deterministic.
pub trait EvalStrategy: Send + 'static {
    /// pick and remove an element from the queue, which is guaranteed to be non-empty
    fn pick_runnable(
        &mut self,
        runnable: &mut VecDeque<(Name, StageResponse)>,
    ) -> (Name, StageResponse);
}

pub struct Fifo;

impl EvalStrategy for Fifo {
    fn pick_runnable(
        &mut self,
        runnable: &mut VecDeque<(Name, StageResponse)>,
    ) -> (Name, StageResponse) {
        runnable
            .pop_front()
            .expect("runnable queue is guaranteed to be non-empty")
    }
}

pub struct RandStdRng(pub StdRng);

impl RandStdRng {
    pub fn derive(&mut self) -> Self {
        RandStdRng(StdRng::seed_from_u64(self.0.random()))
    }
}

impl From<StdRng> for RandStdRng {
    fn from(rng: StdRng) -> Self {
        RandStdRng(rng)
    }
}

impl EvalStrategy for RandStdRng {
    fn pick_runnable(
        &mut self,
        runnable: &mut VecDeque<(Name, StageResponse)>,
    ) -> (Name, StageResponse) {
        let idx = self.0.random_range(0..runnable.len());
        runnable
            .remove(idx)
            .expect("runnable queue is guaranteed to be non-empty")
    }
}
