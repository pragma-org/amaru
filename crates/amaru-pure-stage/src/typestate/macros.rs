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

/// Declare the remainder after `$from` receives `$in`.
///
/// ```ignore
/// on_receive!(Idle, RequestRange => Send<ToPeer, StartBatch> => Streaming | Send<ToPeer, NoBlocks> => Idle);
/// on_receive!(Idle, ClientDone => Done);
/// on_receive!(Busy, StartBatch => Streaming);
/// ```
///
/// The last type of each branch is the next protocol state. Effects before
/// `=>` are consumed by [`Session`](crate::typestate::Session) methods.
#[macro_export]
macro_rules! on_receive {
    ($from:ty, $in:ty => $($then:tt)*) => {
        impl $crate::typestate::OnReceive<$in> for $from {
            type Then = $crate::typestate_par!($($then)*);
        }
    };
}

#[macro_export]
#[doc(hidden)]
macro_rules! typestate_par {
    ($($eff:ty),+ => $s:ty | $($rest:tt)*) => {
        $crate::typestate::Cons<
            $crate::typestate_seq!($($eff),+ => $s),
            $crate::typestate_par!($($rest)*)
        >
    };
    ($s:ty | $($rest:tt)*) => {
        $crate::typestate::Cons<$crate::typestate::To<$s>, $crate::typestate_par!($($rest)*)>
    };
    ($($eff:ty),+ => $s:ty) => {
        $crate::typestate::Cons<$crate::typestate_seq!($($eff),+ => $s), $crate::typestate::Nil>
    };
    ($s:ty) => {
        $crate::typestate::Cons<$crate::typestate::To<$s>, $crate::typestate::Nil>
    };
}

#[macro_export]
#[doc(hidden)]
macro_rules! typestate_seq {
    ($e:ty => $s:ty) => {
        $crate::typestate::Cons<$e, $crate::typestate::To<$s>>
    };
    ($e:ty, $($rest:ty),+ => $s:ty) => {
        $crate::typestate::Cons<$e, $crate::typestate_seq!($($rest),+ => $s)>
    };
}
