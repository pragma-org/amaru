use super::{running::EffectBox, StageEffect};
use std::{
    future::{poll_fn, Future},
    task::Poll,
};

/// The interruption effect factory returned by [`SimulationBuilder::interrupter`](super::SimulationBuilder::interrupter).
#[derive(Clone)]
pub struct Interrupter {
    effect: EffectBox,
}

impl Interrupter {
    pub(crate) fn new(effect: EffectBox) -> Self {
        Self { effect }
    }

    /// Interrupt the current stage and the simulation (if using [`run_until_blocked`](super::SimulationRunning::run_until_blocked)).
    ///
    /// **IMPORTANT:** This method only works when executed from within a stage transition function.
    /// The returned future will never resolve if called from outside a stage transition function.
    pub fn interrupt(&self) -> impl Future<Output = anyhow::Result<()>> + Send + 'static {
        let effect = self.effect.clone();
        let mut todo = true;
        poll_fn(move |_| {
            let mut effect = effect.lock();
            if todo {
                *effect = Some(StageEffect::Interrupt);
                todo = false;
                Poll::Pending
            } else {
                match &*effect {
                    Some(StageEffect::Interrupt) => Poll::Pending,
                    Some(x) => Poll::Ready(Err(anyhow::anyhow!("unexpected effect {:?}", x))),
                    None => Poll::Ready(Ok(())),
                }
            }
        })
    }
}
