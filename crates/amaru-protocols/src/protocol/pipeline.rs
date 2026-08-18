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

//! Round-robin cursors for CIP-0164-style pipelining.
//!
//! One handler stage owns [`Pipeline`] plus a `Vec` of lock-step instances.
//! This module does not send messages; the handler calls it and then talks to
//! the mux. Index selection is not type-checked.

use std::num::NonZeroUsize;

/// An instance changed its occupancy of the switch state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum SwitchCredit {
    /// Left the switch state. Instance now has remote agency (or is terminal after Close).
    Left,
    /// Entered the switch state. Instance now has local agency.
    Entered,
    /// Stayed off the switch state. Instance still has remote agency.
    Stay,
    /// Reached a terminal protocol state.
    Terminated,
}

/// Result of trying to admit a node request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admit<Req> {
    /// Reserved instance `0..n` and advanced `send_idx`.
    Instance(usize),
    /// All instances reserved; request stored as the replaceable slack slot.
    Slack,
    /// Slack was occupied; the previous unsent request is returned.
    ReplacedSlack(Req),
    /// Closing or closed; request dropped.
    Dropped,
}

/// What the handler should do after a credit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorHint {
    None,
    WantNext,
}

/// What the handler should do after `Close`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseHint {
    /// Inject Close into this idle instance (once).
    Inject(usize),
    /// Wait for reserved instances to re-enter the switch state.
    Drain,
    /// Close already injected or the pipeline is closed.
    Already,
}

/// Cursor / admission error. The handler must terminate the connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PipelineError {
    /// Credit named an instance that is not `0..n`.
    InstanceOutOfRange { instance: usize, n: usize },
    /// `Stay`/`Entered` from an instance that is not the receive cursor.
    UnexpectedReceiveInstance { instance: usize, recv_idx: usize },
    /// `Terminated` from an instance that was not injected with Close.
    UnexpectedTerminated { instance: usize },
    /// Credit applied to a terminated instance.
    AlreadyTerminated { instance: usize },
}

impl std::fmt::Display for PipelineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InstanceOutOfRange { instance, n } => {
                write!(f, "pipeline instance {instance} is out of range (n={n})")
            }
            Self::UnexpectedReceiveInstance { instance, recv_idx } => {
                write!(f, "pipeline receive credit from instance {instance}, expected {recv_idx}")
            }
            Self::UnexpectedTerminated { instance } => {
                write!(f, "pipeline terminated credit from instance {instance} without Close inject")
            }
            Self::AlreadyTerminated { instance } => {
                write!(f, "pipeline credit from already terminated instance {instance}")
            }
        }
    }
}

impl std::error::Error for PipelineError {}

/// Send/receive cursors, slack, and Close inject-once.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Pipeline<Req> {
    n: usize,
    send_idx: usize,
    recv_idx: usize,
    idle: Vec<bool>,
    remote: Vec<bool>,
    terminated: Vec<bool>,
    pending: Option<Req>,
    pending_close: bool,
    close_injected: Option<usize>,
    closed: bool,
    want_inflight: bool,
    registered: bool,
}

impl<Req> Pipeline<Req> {
    pub fn new(n: NonZeroUsize) -> Self {
        let n = n.get();
        Self {
            n,
            send_idx: 0,
            recv_idx: 0,
            idle: vec![true; n],
            remote: vec![false; n],
            terminated: vec![false; n],
            pending: None,
            pending_close: false,
            close_injected: None,
            closed: false,
            want_inflight: false,
            registered: false,
        }
    }

    pub fn n(&self) -> usize {
        self.n
    }

    pub fn send_idx(&self) -> usize {
        self.send_idx
    }

    pub fn recv_idx(&self) -> usize {
        self.recv_idx
    }

    pub fn mark_registered(&mut self) {
        self.registered = true;
    }

    pub fn is_registered(&self) -> bool {
        self.registered
    }

    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// `WantNext` is legal iff the mux is registered, no pull is in flight, and
    /// the receive cursor's instance has remote agency.
    pub fn should_want_next(&self) -> bool {
        self.registered && !self.want_inflight && !self.closed && self.remote[self.recv_idx]
    }

    pub fn mark_want_sent(&mut self) {
        self.want_inflight = true;
    }

    pub fn mark_want_consumed(&mut self) {
        self.want_inflight = false;
    }

    /// Reserve `send_idx` if it is idle, otherwise store or replace slack.
    pub fn try_admit(&mut self, req: Req) -> Admit<Req> {
        if self.pending_close || self.close_injected.is_some() || self.closed {
            return Admit::Dropped;
        }
        if self.idle[self.send_idx] && !self.terminated[self.send_idx] {
            let i = self.send_idx;
            self.idle[i] = false;
            self.send_idx = (self.send_idx + 1) % self.n;
            return Admit::Instance(i);
        }
        match self.pending.replace(req) {
            None => Admit::Slack,
            Some(old) => Admit::ReplacedSlack(old),
        }
    }

    /// If slack is waiting and `send_idx` is idle, take it for a follow-up admit.
    pub fn take_slack_if_ready(&mut self) -> Option<Req> {
        if self.pending_close || self.close_injected.is_some() || self.closed {
            return None;
        }
        if self.idle[self.send_idx] && !self.terminated[self.send_idx] { self.pending.take() } else { None }
    }

    pub fn on_credit(&mut self, instance: usize, credit: SwitchCredit) -> Result<CursorHint, PipelineError> {
        self.check_instance(instance)?;
        if self.terminated[instance] {
            return Err(PipelineError::AlreadyTerminated { instance });
        }
        match credit {
            SwitchCredit::Left => {
                self.remote[instance] = true;
            }
            SwitchCredit::Stay => {
                if instance != self.recv_idx {
                    return Err(PipelineError::UnexpectedReceiveInstance { instance, recv_idx: self.recv_idx });
                }
            }
            SwitchCredit::Entered => {
                if instance != self.recv_idx {
                    return Err(PipelineError::UnexpectedReceiveInstance { instance, recv_idx: self.recv_idx });
                }
                self.idle[instance] = true;
                self.remote[instance] = false;
                self.recv_idx = (self.recv_idx + 1) % self.n;
            }
            SwitchCredit::Terminated => {
                if self.close_injected != Some(instance) {
                    return Err(PipelineError::UnexpectedTerminated { instance });
                }
                self.terminated[instance] = true;
                self.closed = true;
                self.remote[instance] = false;
                return Ok(CursorHint::None);
            }
        }
        Ok(self.hint())
    }

    /// Drop slack and inject Close once every reserved instance is idle again.
    pub fn on_close(&mut self) -> CloseHint {
        if self.close_injected.is_some() || self.closed {
            return CloseHint::Already;
        }
        self.pending = None;
        self.pending_close = true;
        self.try_inject_close()
    }

    /// After an `Entered` during drain, try to inject Close.
    pub fn try_inject_close(&mut self) -> CloseHint {
        if self.close_injected.is_some() || self.closed {
            return CloseHint::Already;
        }
        if !self.pending_close {
            return CloseHint::Drain;
        }
        if !(0..self.n).all(|i| self.idle[i] || self.terminated[i]) {
            return CloseHint::Drain;
        }
        let i = self.send_idx;
        self.close_injected = Some(i);
        self.idle[i] = false;
        CloseHint::Inject(i)
    }

    fn hint(&self) -> CursorHint {
        if self.should_want_next() { CursorHint::WantNext } else { CursorHint::None }
    }

    fn check_instance(&self, instance: usize) -> Result<(), PipelineError> {
        if instance < self.n { Ok(()) } else { Err(PipelineError::InstanceOutOfRange { instance, n: self.n }) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n2() -> Pipeline<&'static str> {
        Pipeline::new(NonZeroUsize::new(2).expect("2"))
    }

    #[test]
    fn first_two_admits_reserve_distinct_instances() {
        let mut p = n2();
        assert_eq!(p.try_admit("A"), Admit::Instance(0));
        assert_eq!(p.try_admit("B"), Admit::Instance(1));
        assert_eq!(p.send_idx(), 0);
        assert_eq!(p.try_admit("C"), Admit::Slack);
        assert_eq!(p.try_admit("D"), Admit::ReplacedSlack("C"));
    }

    #[test]
    fn no_want_next_until_registered_and_left() {
        let mut p = n2();
        assert!(!p.should_want_next());
        assert_eq!(p.try_admit("A"), Admit::Instance(0));
        assert_eq!(p.on_credit(0, SwitchCredit::Left).unwrap(), CursorHint::None);
        p.mark_registered();
        assert_eq!(p.on_credit(0, SwitchCredit::Stay).unwrap(), CursorHint::WantNext);
        p.mark_want_sent();
        assert!(!p.should_want_next());
        p.mark_want_consumed();
        assert!(p.should_want_next());
    }

    #[test]
    fn entered_advances_recv_and_flushes_slack() {
        let mut p = n2();
        p.mark_registered();
        assert_eq!(p.try_admit("A"), Admit::Instance(0));
        assert_eq!(p.try_admit("B"), Admit::Instance(1));
        assert_eq!(p.try_admit("C"), Admit::Slack);
        p.on_credit(0, SwitchCredit::Left).unwrap();
        p.on_credit(1, SwitchCredit::Left).unwrap();
        p.mark_want_sent();
        p.mark_want_consumed();
        assert_eq!(p.on_credit(0, SwitchCredit::Entered).unwrap(), CursorHint::WantNext);
        assert_eq!(p.recv_idx(), 1);
        assert_eq!(p.take_slack_if_ready(), Some("C"));
        assert_eq!(p.try_admit("C"), Admit::Instance(0));
    }

    #[test]
    fn stay_from_wrong_instance_is_an_error() {
        let mut p = n2();
        p.try_admit("A");
        p.try_admit("B");
        p.on_credit(0, SwitchCredit::Left).unwrap();
        p.on_credit(1, SwitchCredit::Left).unwrap();
        assert_eq!(
            p.on_credit(1, SwitchCredit::Stay),
            Err(PipelineError::UnexpectedReceiveInstance { instance: 1, recv_idx: 0 })
        );
    }

    #[test]
    fn close_after_one_reservation_injects_the_unused_instance() {
        let mut p = n2();
        assert_eq!(p.try_admit("A"), Admit::Instance(0));
        assert_eq!(p.on_close(), CloseHint::Drain);
        p.on_credit(0, SwitchCredit::Left).unwrap();
        p.on_credit(0, SwitchCredit::Entered).unwrap();
        assert_eq!(p.try_inject_close(), CloseHint::Inject(1));
        assert_eq!(p.try_inject_close(), CloseHint::Already);
        assert_eq!(p.on_close(), CloseHint::Already);
        assert_eq!(p.on_credit(1, SwitchCredit::Terminated).unwrap(), CursorHint::None);
        assert!(p.is_closed());
        assert_eq!(p.try_admit("X"), Admit::Dropped);
    }

    #[test]
    fn close_drops_slack_then_drains_reserved() {
        let mut p = n2();
        assert_eq!(p.try_admit("A"), Admit::Instance(0));
        assert_eq!(p.try_admit("B"), Admit::Instance(1));
        assert_eq!(p.try_admit("C"), Admit::Slack);
        assert_eq!(p.on_close(), CloseHint::Drain);
        assert_eq!(p.take_slack_if_ready(), None);
        p.on_credit(0, SwitchCredit::Left).unwrap();
        p.on_credit(0, SwitchCredit::Entered).unwrap();
        assert_eq!(p.try_inject_close(), CloseHint::Drain);
        p.on_credit(1, SwitchCredit::Left).unwrap();
        p.on_credit(1, SwitchCredit::Entered).unwrap();
        assert_eq!(p.try_inject_close(), CloseHint::Inject(0));
    }

    #[test]
    fn terminated_without_inject_is_an_error() {
        let mut p = n2();
        assert_eq!(p.on_credit(0, SwitchCredit::Terminated), Err(PipelineError::UnexpectedTerminated { instance: 0 }));
    }
}
