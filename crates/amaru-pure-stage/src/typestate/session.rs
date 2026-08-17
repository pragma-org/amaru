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

use std::{fmt, future::Future, marker::PhantomData, time::Duration};

use super::{
    Clean, FmtPar, Role, RoleTag, Select,
    effect::{Send as SendEff, Terminate, Wait},
    list,
};
use crate::{Effects, ExternalEffectAPI, Instant, SendData, StageRef};

/// Witness that only [`initial_state`] and [`Session::finish`] may construct a protocol state.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Marker(Private);

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) struct Private;

pub struct InitialState;
pub struct NotInitialState;

/// A zero-sized protocol state. Construct the initial one with [`initial_state`].
pub trait State: Sized + Send + 'static {
    const NAME: &'static str;
    /// Used by [`initial_state`] and [`Session::finish`] only.
    #[doc(hidden)]
    fn make(marker: Marker) -> Self;
    type Initial;

    /// Consume the receive allowance for `input` and return the remaining effects.
    ///
    /// `input` need not be the stage mailbox type. It is typically one variant
    /// (or a newtype of a variant) so different messages select different
    /// [`OnReceive`] impls and therefore different remainders.
    ///
    /// ```compile_fail
    /// use amaru_pure_stage::typestate::prelude::*;
    /// make_states!(Live { Idle; Done });
    /// define_role_tag!(ToPeer);
    /// on_receive!(Idle, u8 => Send<ToPeer, String> => Done);
    /// fn bad<M>(s: Idle, eff: amaru_pure_stage::Effects<M>) {
    ///     let _ = s.receive(true, eff);
    /// }
    /// ```
    fn receive<In, M>(self, input: In, eff: Effects<M>) -> Session<M, <Self as OnReceive<In>>::Then>
    where
        Self: OnReceive<In>,
    {
        <Self as OnReceive<In>>::open(self, input, eff)
    }
}

/// The remainder after [`State::receive`] of `In`.
pub trait OnReceive<In>: State {
    type Then;

    fn open<M>(self, input: In, eff: Effects<M>) -> Session<M, Self::Then> {
        let _ = (self, input);
        Session::new(eff)
    }
}

/// Construct the unique initial state of a protocol. The only other way to
/// obtain a state value is [`Session::finish`].
pub fn initial_state<S>() -> S
where
    S: State<Initial = InitialState>,
{
    S::make(Marker(Private))
}

/// Terminator of an effect sequence: after the preceding effects, the session
/// finishes in `S`.
pub struct To<S: State>(PhantomData<S>);

/// `Effects` plus the type-level remainder of a receive.
///
/// Protocol [`send`](Self::send), [`wait`](Self::wait), and
/// [`terminate`](Self::terminate) consume from `Rem`. Local decision helpers
/// (`clock`, `external`, `notify`) do not.
pub struct Session<M, Rem> {
    effects: Effects<M>,
    _rem: PhantomData<fn() -> Rem>,
}

impl<M, Rem> Session<M, Rem> {
    fn new(effects: Effects<M>) -> Self {
        Self { effects, _rem: PhantomData }
    }

    /// Human-readable remainder (`Send<Role, A> => S | Send<Role, B> => T`).
    pub fn describe() -> String
    where
        Rem: FmtPar,
    {
        list::describe::<Rem>()
    }

    /// Underlying [`Effects`] for local decisions. Do not use this to send
    /// protocol messages; that bypasses the remainder.
    pub fn inner(&self) -> &Effects<M> {
        &self.effects
    }

    pub fn me(&self) -> StageRef<M>
    where
        M: SendData,
    {
        self.effects.me()
    }

    pub fn clock(&self) -> crate::BoxFuture<'static, Instant> {
        self.effects.clock()
    }

    pub fn external<T: ExternalEffectAPI>(&self, effect: T) -> crate::BoxFuture<'static, T::Response> {
        self.effects.external(effect)
    }

    /// Send a message that is *not* part of the protocol remainder (local
    /// plumbing, metrics, self-replies).
    pub fn notify<T: SendData>(&self, target: &StageRef<T>, msg: T) -> crate::BoxFuture<'static, ()> {
        self.effects.send(target, msg)
    }

    /// Protocol send. Consumes a [`Send<Tag, T>`] allowance.
    ///
    /// `target` wraps a [`StageRef`] claiming `Tag`; its mailbox must implement [`From<T>`].
    ///
    /// ```compile_fail
    /// use amaru_pure_stage::typestate::prelude::*;
    /// make_states!(Live { Idle; Done });
    /// define_role_tag!(ToPeer);
    /// define_role!(Peer, ToPeer, String);
    /// on_receive!(Idle, u8 => Send<ToPeer, String> => Done);
    /// async fn bad<M>(s: Idle, target: &Peer, eff: amaru_pure_stage::Effects<M>) {
    ///     let _ = s.receive(1u8, eff).send(target, 0u32).await;
    /// }
    /// ```
    pub fn send<Tag, T, Dest, I>(
        self,
        target: &Dest,
        msg: T,
    ) -> impl Future<Output = Session<M, After<Rem, SendEff<Tag, T>, I>>> + Send
    where
        Tag: RoleTag,
        Dest: Role<Tag>,
        Dest::Mailbox: From<T>,
        Rem: Select<SendEff<Tag, T>, I>,
        Rem::Rest: Clean,
        M: Send,
    {
        let send = self.effects.send(target.mailbox(), Dest::Mailbox::from(msg));
        async move {
            send.await;
            Session::new(self.effects)
        }
    }

    /// Protocol wait. Consumes a [`Wait`] allowance.
    pub fn wait<I>(self, delay: Duration) -> impl Future<Output = (Instant, Session<M, After<Rem, Wait, I>>)> + Send
    where
        Rem: Select<Wait, I>,
        Rem::Rest: Clean,
        M: Send,
    {
        let wait = self.effects.wait(delay);
        async move {
            let now = wait.await;
            (now, Session::new(self.effects))
        }
    }

    /// Protocol terminate. Consumes a [`Terminate`] allowance. Never returns.
    pub fn terminate<T, I>(self) -> impl Future<Output = T> + Send
    where
        Rem: Select<Terminate, I>,
        T: Send,
    {
        self.effects.terminate()
    }

    /// End the session when a remainder branch is [`To<S>`].
    ///
    /// ```compile_fail
    /// use amaru_pure_stage::typestate::prelude::*;
    /// make_states!(Live { Idle; Done });
    /// define_role_tag!(ToPeer);
    /// on_receive!(Idle, u8 => Send<ToPeer, String> => Done);
    /// fn bad<M>(s: Idle, eff: amaru_pure_stage::Effects<M>) -> Done {
    ///     s.receive(1u8, eff).finish()
    /// }
    /// ```
    pub fn finish<S: State, I>(self) -> S
    where
        Rem: Select<To<S>, I>,
    {
        let _ = self;
        S::make(Marker(Private))
    }
}

impl<M, Rem: FmtPar> fmt::Debug for Session<M, Rem> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Session").field("remaining", &list::describe::<Rem>()).finish_non_exhaustive()
    }
}

type After<Rem, E, I> = <<Rem as Select<E, I>>::Rest as Clean>::Out;

/// Describe `State + Receive<In> → remainder` without constructing values.
pub fn describe_receive<S, In>() -> String
where
    S: OnReceive<In>,
    S::Then: FmtPar,
{
    format!("{} + Receive<{}> → {}", S::NAME, std::any::type_name::<In>(), list::describe::<S::Then>())
}
