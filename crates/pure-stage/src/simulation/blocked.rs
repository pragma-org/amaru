use crate::{Effect, Name};

/// Classification of why [`SimulationRunning::run_until_blocked`](crate::simulation::SimulationRunning::run_until_blocked) has stopped.
#[derive(Debug, PartialEq)]
pub enum Blocked {
    /// All stages are suspended on [`Effect::Receive`].
    Idle,
    /// The simulation is waiting for a wakeup.
    Sleeping,
    /// All stages are suspended on either [`Effect::Receive`] or [`Effect::Send`].
    Deadlock(Vec<Name>),
    /// The given breakpoint was hit.
    Breakpoint(Name, Effect),
    /// The given stages are suspended on effects other than [`Effect::Receive`]
    /// while none are suspended on [`Effect::Send`].
    Busy(Vec<Name>),
}

impl Blocked {
    /// Assert that the blocking reason is `Idle`.
    pub fn assert_idle(&self) {
        match self {
            Blocked::Idle => {}
            _ => panic!("expected idle, got {:?}", self),
        }
    }

    /// Assert that the blocking reason is `Sleeping`.
    pub fn assert_sleeping(&self) {
        match self {
            Blocked::Sleeping => {}
            _ => panic!("expected sleeping, got {:?}", self),
        }
    }

    /// Assert that the blocking reason is `Deadlock` by at least the given stages.
    pub fn assert_deadlock(&self, names: impl IntoIterator<Item = impl AsRef<str>>) {
        let names = names
            .into_iter()
            .map(|n| Name::from(n.as_ref()))
            .collect::<Vec<_>>();
        match self {
            Blocked::Deadlock(deadlock) if names.iter().all(|n| deadlock.contains(n)) => {}
            _ => panic!("expected deadlock by {:?}, got {:?}", names, self),
        }
    }

    /// Assert that the blocking reason is `Busy` by at least the given stages.
    pub fn assert_busy(&self, names: impl IntoIterator<Item = impl AsRef<str>>) {
        let names = names
            .into_iter()
            .map(|n| Name::from(n.as_ref()))
            .collect::<Vec<_>>();
        match self {
            Blocked::Busy(busy) if names.iter().all(|n| busy.contains(n)) => {}
            _ => panic!("expected busy by {:?}, got {:?}", names, self),
        }
    }

    /// Assert that the blocking reason is `Breakpoint` by the given name.
    pub fn assert_breakpoint(self, name: impl AsRef<str>) -> Effect {
        match self {
            Blocked::Breakpoint(n, eff) if n.as_str() == name.as_ref() => eff,
            _ => panic!("expected breakpoint `{}`, got {:?}", name.as_ref(), self),
        }
    }
}
