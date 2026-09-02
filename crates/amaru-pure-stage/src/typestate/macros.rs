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

//! Surface syntax for states, remainders, roles, and mailboxes.
//!
//! `typestate_par!` splits on the first `=> State`: `|` before that is parallel
//! sequences (one [`Then`](crate::typestate::Then)); `|` after starts another
//! choice alternative (n-way `Cons` of `Then`). A lone ident is `Then<Nil, S>`.
//! [`star`]`(A, B)` is `Repeat` of that sequence. Grouped [`on_receive`] builds
//! the input enum and [`ExtractInput`](crate::typestate::ExtractInput).

/// Declare protocol states and the live enum that holds them.
///
/// Names before `;` are initial (constructible via
/// [`initial_state`](crate::typestate::initial_state)); the rest are only
/// produced by [`Session::finish`](crate::typestate::Session::finish). Each
/// enum variant holds the matching typed state; wrap `finish()` / `initial_state()`
/// with [`Into`].
///
/// ```ignore
/// make_states!(Live { Idle; Busy, Done });
/// // Live::from(initial_state::<Idle>())
/// // Live::from(session.finish())
/// ```
#[macro_export]
macro_rules! make_states {
    ($vis:vis $enum:ident { $($init:ident),+ $(,)?; $($other:ident),+ $(,)? }) => {
        $crate::typestate_state_structs!($vis $($init),+ ; $($other),+);
        $crate::typestate_live_enum!($vis $enum { $($init),+, $($other),+ });
    };
    ($vis:vis $enum:ident { $($init:ident),+ $(,)? }) => {
        $crate::typestate_state_structs!($vis $($init),+);
        $crate::typestate_live_enum!($vis $enum { $($init),+ });
    };
}

#[macro_export]
#[doc(hidden)]
macro_rules! typestate_state_structs {
    ($vis:vis $($init:ident),+ ; $($other:ident),+) => {
        $(
            #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
            $vis struct $init($crate::typestate::Marker);
            impl $crate::typestate::State for $init {
                const NAME: &'static str = stringify!($init);
                fn make(marker: $crate::typestate::Marker) -> Self {
                    $init(marker)
                }
                type Initial = $crate::typestate::InitialState;
            }
        )*
        $(
            #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
            $vis struct $other($crate::typestate::Marker);
            impl $crate::typestate::State for $other {
                const NAME: &'static str = stringify!($other);
                fn make(marker: $crate::typestate::Marker) -> Self {
                    $other(marker)
                }
                type Initial = $crate::typestate::NotInitialState;
            }
        )*
    };
    ($vis:vis $($init:ident),+) => {
        $(
            #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
            $vis struct $init($crate::typestate::Marker);
            impl $crate::typestate::State for $init {
                const NAME: &'static str = stringify!($init);
                fn make(marker: $crate::typestate::Marker) -> Self {
                    $init(marker)
                }
                type Initial = $crate::typestate::InitialState;
            }
        )*
    };
}

#[macro_export]
#[doc(hidden)]
macro_rules! typestate_live_enum {
    ($vis:vis $name:ident { $($state:ident),+ $(,)? }) => {
        #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
        $vis enum $name {
            $($state($state),)+
        }

        $(
            impl ::core::convert::From<$state> for $name {
                fn from(state: $state) -> Self {
                    $name::$state(state)
                }
            }
        )+
    };
}

/// A ZST tag naming a send destination (`Send<$name, T>`).
#[macro_export]
macro_rules! define_role_tag {
    ($vis:vis $name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        $vis struct $name;

        impl $crate::typestate::RoleTag for $name {
            const NAME: &'static str = stringify!($name);
        }
    };
}

/// A [`StageRef<$mailbox>`](crate::StageRef) wrapper that claims role tag `$tag`.
///
/// [`Send<$tag, T>`](crate::typestate::Send) is usable with a `$name` value
/// when `$mailbox: From<T>`.
#[macro_export]
macro_rules! define_role {
    ($vis:vis $name:ident, $tag:ty, $mailbox:ty) => {
        #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
        $vis struct $name($crate::StageRef<$mailbox>);

        impl $name {
            pub fn new(stage: $crate::StageRef<$mailbox>) -> Self {
                Self(stage)
            }
        }

        impl $crate::typestate::Role<$tag> for $name {
            type Mailbox = $mailbox;

            fn mailbox(&self) -> &$crate::StageRef<$mailbox> {
                &self.0
            }
        }
    };
}

/// A stage mailbox: uniquely named cases wrapping protocol payloads.
///
/// Generates [`From`] into the mailbox (for [`Send`](crate::typestate::Send))
/// and [`FromMailbox`](crate::typestate::FromMailbox) the other way (for
/// [`convert_input`](crate::typestate::State::convert_input)).
#[macro_export]
macro_rules! define_mailbox {
    ($vis:vis $name:ident { $($var:ident ($ty:ty)),+ $(,)? }) => {
        #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
        $vis enum $name {
            $($var($ty),)+
        }

        $(
            impl ::core::convert::From<$ty> for $name {
                fn from(value: $ty) -> Self {
                    $name::$var(value)
                }
            }

            #[allow(clippy::wildcard_enum_match_arm)]
            impl $crate::typestate::FromMailbox<$name> for $ty {
                fn from_mailbox(msg: $name) -> ::core::result::Result<$ty, $name> {
                    match msg {
                        $name::$var(value) => ::core::result::Result::Ok(value),
                        other => ::core::result::Result::Err(other),
                    }
                }
            }
        )+
    };
}

/// Declare the remainder after `$from` receives `$in`.
///
/// Grouped form builds an input enum and [`convert_input`](crate::typestate::State::convert_input):
///
/// ```ignore
/// on_receive!(Idle as IdleIn {
///     RequestRange => {
///         Send<ToPeer, StartBatch> => Streaming
///         | Send<ToPeer, NoBlocks> => Idle
///         | Repeat<SendAny<ToSelf>>
///     }
///     ClientDone => { Done }
/// });
/// ```
///
/// `|` *before* `=> State` is parallel composition: every branch must be
/// discharged, then the single next state is taken. `|` *between*
/// `=> State` groups is choice of next state.
///
/// Single-input form only implements [`OnReceive`](crate::typestate::OnReceive)
/// (used for type-level descriptions).
#[macro_export]
macro_rules! on_receive {
    ($from:ident as $inputs:ident { $($body:tt)* }) => {
        $crate::typestate_on_receive_body!($from, $inputs, [], $($body)*);
    };
    ($from:ty, $in:ty => $($then:tt)*) => {
        impl $crate::typestate::OnReceive<$in> for $from {
            type Then = $crate::typestate_par!($($then)*);
        }
    };
}

#[macro_export]
#[doc(hidden)]
macro_rules! typestate_on_receive_body {
    ($from:ident, $inputs:ident, [$($done:tt)*], $in:ident => { $($then:tt)* } $($rest:tt)*) => {
        $crate::typestate_on_receive_body!($from, $inputs, [$($done)* [$in, $($then)*]], $($rest)*);
    };
    ($from:ident, $inputs:ident, [$([$in:ident, $($then:tt)*])*] $(,)?) => {
        #[derive(Debug, Clone, PartialEq, Eq)]
        enum $inputs {
            $($in($in),)*
        }

        impl<M> $crate::typestate::ExtractInput<M> for $inputs
        where
            $($in: $crate::typestate::FromMailbox<M>,)*
        {
            fn extract(msg: M) -> ::core::result::Result<Self, M> {
                $crate::typestate_extract!(msg, $inputs, $($in),*)
            }
        }

        $(
            impl $crate::typestate::OnReceive<$in> for $from {
                type Then = $crate::typestate_par!($($then)*);
            }
        )*
    };
}

#[macro_export]
#[doc(hidden)]
macro_rules! typestate_extract {
    ($msg:ident, $enum:ident, ) => {
        ::core::result::Result::Err($msg)
    };
    ($msg:ident, $enum:ident, $in:ident) => {
        match <$in as $crate::typestate::FromMailbox<_>>::from_mailbox($msg) {
            ::core::result::Result::Ok(value) => ::core::result::Result::Ok($enum::$in(value)),
            ::core::result::Result::Err(msg) => ::core::result::Result::Err(msg),
        }
    };
    ($msg:ident, $enum:ident, $in:ident, $($rest:ident),+) => {
        match <$in as $crate::typestate::FromMailbox<_>>::from_mailbox($msg) {
            ::core::result::Result::Ok(value) => ::core::result::Result::Ok($enum::$in(value)),
            ::core::result::Result::Err(msg) => $crate::typestate_extract!(msg, $enum, $($rest),+),
        }
    };
}

#[macro_export]
#[doc(hidden)]
macro_rules! typestate_par {
    ($s:ident) => {
        $crate::typestate::Cons<
            $crate::typestate::Then<$crate::typestate::Nil, $s>,
            $crate::typestate::Nil
        >
    };
    ($($rest:tt)*) => {
        $crate::typestate_choice!(@go [] [] $($rest)*)
    };
}

#[macro_export]
#[doc(hidden)]
macro_rules! typestate_choice {
    (@go [ $($seqs:tt)* ] [ $($cur:ty),* ] $ty:ty, $($rest:tt)*) => {
        $crate::typestate_choice!(@go [ $($seqs)* ] [ $($cur,)* $ty ] $($rest)*)
    };
    (@go [ $($seqs:tt)* ] [ $($cur:ty),* ] $ty:ty | $($rest:tt)*) => {
        $crate::typestate_choice!(@go [ $($seqs)* [ $($cur,)* $ty ] ] [] $($rest)*)
    };
    (@go [ $($seqs:tt)* ] [ $($cur:ty),* ] $ty:ty => $s:ident | $($rest:tt)*) => {
        $crate::typestate::Cons<
            $crate::typestate::Then<
                $crate::typestate_branches!([ $($seqs)* [ $($cur,)* $ty ] ]),
                $s
            >,
            $crate::typestate_par!($($rest)*)
        >
    };
    (@go [ $($seqs:tt)* ] [ $($cur:ty),* ] $ty:ty => $s:ident) => {
        $crate::typestate::Cons<
            $crate::typestate::Then<
                $crate::typestate_branches!([ $($seqs)* [ $($cur,)* $ty ] ]),
                $s
            >,
            $crate::typestate::Nil
        >
    };
}

#[macro_export]
#[doc(hidden)]
macro_rules! typestate_branches {
    ([]) => {
        $crate::typestate::Nil
    };
    ([ [ $($ty:ty),+ ] $($rest:tt)* ]) => {
        $crate::typestate::Cons<
            $crate::typestate_seq!($($ty),+),
            $crate::typestate_branches!([ $($rest)* ])
        >
    };
}

#[macro_export]
#[doc(hidden)]
macro_rules! typestate_seq {
    ($e:ty) => {
        $crate::typestate::Cons<$e, $crate::typestate::Nil>
    };
    ($e:ty, $($rest:ty),+) => {
        $crate::typestate::Cons<$e, $crate::typestate_seq!($($rest),+)>
    };
}

/// `Repeat` of a sequence: `star!(Send<A, T>, Send<B, U>)`.
#[macro_export]
macro_rules! star {
    ($($e:ty),+) => {
        $crate::typestate::Repeat<$crate::typestate_seq!($($e),+)>
    };
}
