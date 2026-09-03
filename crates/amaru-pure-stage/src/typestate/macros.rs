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
//! [`define_messages`](crate::define_messages) generates a struct per variant plus [`From`] /
//! [`FromMailbox`](crate::typestate::FromMailbox); [`define_mailbox`](crate::define_mailbox) wraps
//! already-defined payload types.

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
///
/// make_states!(Live { Idle; Busy, Done } switch Idle, terminal Done);
/// // OccupancyOf: Idle is Switch, Done is Terminal, Busy is Remote
/// ```
///
/// After the brace, `switch $State` names the CIP-0164 switch state (need not
/// be initial). `terminal $State` names a state with no agency. Remaining
/// variants are remote agency. [`OccupancyOf`](crate::typestate::OccupancyOf)
/// is generated only when `switch` is present.
#[macro_export]
macro_rules! make_states {
    ($vis:vis $enum:ident { $($init:ident),+ $(,)?; $($other:ident),+ $(,)? } switch $switch:ident, terminal $term:ident) => {
        $crate::typestate_state_structs!($vis $($init),+ ; $($other),+);
        $crate::typestate_live_enum!($vis $enum { $($init),+, $($other),+ });
        $crate::typestate_occupancy!($enum, $switch, $term);
    };
    ($vis:vis $enum:ident { $($init:ident),+ $(,)?; $($other:ident),+ $(,)? } switch $switch:ident) => {
        $crate::typestate_state_structs!($vis $($init),+ ; $($other),+);
        $crate::typestate_live_enum!($vis $enum { $($init),+, $($other),+ });
        $crate::typestate_occupancy!($enum, $switch);
    };
    ($vis:vis $enum:ident { $($init:ident),+ $(,)?; $($other:ident),+ $(,)? }) => {
        $crate::typestate_state_structs!($vis $($init),+ ; $($other),+);
        $crate::typestate_live_enum!($vis $enum { $($init),+, $($other),+ });
    };
    ($vis:vis $enum:ident { $($init:ident),+ $(,)? } switch $switch:ident, terminal $term:ident) => {
        $crate::typestate_state_structs!($vis $($init),+);
        $crate::typestate_live_enum!($vis $enum { $($init),+ });
        $crate::typestate_occupancy!($enum, $switch, $term);
    };
    ($vis:vis $enum:ident { $($init:ident),+ $(,)? } switch $switch:ident) => {
        $crate::typestate_state_structs!($vis $($init),+);
        $crate::typestate_live_enum!($vis $enum { $($init),+ });
        $crate::typestate_occupancy!($enum, $switch);
    };
    ($vis:vis $enum:ident { $($init:ident),+ $(,)? }) => {
        $crate::typestate_state_structs!($vis $($init),+);
        $crate::typestate_live_enum!($vis $enum { $($init),+ });
    };
}

#[macro_export]
#[doc(hidden)]
macro_rules! typestate_occupancy {
    ($enum:ident, $switch:ident, $term:ident) => {
        impl $crate::typestate::OccupancyOf for $enum {
            fn occupancy(&self) -> $crate::typestate::Occupancy {
                #[allow(clippy::wildcard_enum_match_arm)]
                match self {
                    Self::$switch(_) => $crate::typestate::Occupancy::Switch,
                    Self::$term(_) => $crate::typestate::Occupancy::Terminal,
                    _ => $crate::typestate::Occupancy::Remote,
                }
            }
        }
    };
    ($enum:ident, $switch:ident) => {
        impl $crate::typestate::OccupancyOf for $enum {
            fn occupancy(&self) -> $crate::typestate::Occupancy {
                #[allow(clippy::wildcard_enum_match_arm)]
                match self {
                    Self::$switch(_) => $crate::typestate::Occupancy::Switch,
                    _ => $crate::typestate::Occupancy::Remote,
                }
            }
        }
    };
}

#[macro_export]
#[doc(hidden)]
macro_rules! typestate_state_structs {
    ($vis:vis $($init:ident),+ ; $($other:ident),+) => {
        $(
            #[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
            #[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
            #[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
        #[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
/// [`convert_input`](crate::typestate::State::convert_input)). Extra attributes
/// (including additional `#[derive]`) are placed on the enum beside the
/// default `Debug, Clone, PartialEq, Eq, Serialize, Deserialize`.
///
/// When the payloads do not yet exist as types, use [`define_messages`](crate::define_messages)
/// instead: it generates a struct per variant and the same conversions.
#[macro_export]
macro_rules! define_mailbox {
    ($(#[$attr:meta])* $vis:vis $name:ident { $($var:ident ($ty:ty)),+ $(,)? }) => {
        $(#[$attr])*
        #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
        $vis enum $name {
            $($var($ty),)+
        }

        $crate::define_mailbox_conversions!($name { $($var ($ty)),+ });
    };
}

/// Declare a protocol message enum together with a struct per variant.
///
/// Each variant becomes a type of the same name. The enum wraps those
/// structs (`Message::RequestRange(RequestRange { .. })`). [`From`] into
/// the enum and [`FromMailbox`](crate::typestate::FromMailbox) the other
/// way are generated so the variants work with [`Send`](crate::typestate::Send)
/// and [`convert_input`](crate::typestate::State::convert_input).
///
/// `#[derive(...)]` on the enum is applied to the enum **and** every
/// variant struct. Other attributes and doc comments on the enum stay on
/// the enum. Attributes on a variant (including extra `#[derive]`) apply
/// only to that struct. Further impls (`Encode`, `message_type`, …) can
/// be written as usual after the macro.
///
/// ```ignore
/// define_messages! {
///     #[derive(Debug, Clone, PartialEq, Eq)]
///     pub enum Message {
///         RequestRange { from: Point, through: Point },
///         #[derive(Copy)]
///         ClientDone,
///         Block(Vec<u8>),
///     }
/// }
/// // RequestRange { from, through }.into() == Message::RequestRange(...)
/// ```
#[macro_export]
macro_rules! define_messages {
    (
        $(#[$attr:meta])*
        $vis:vis enum $name:ident {
            $($body:tt)*
        }
    ) => {
        $crate::define_messages_emit! {
            attrs = [$(#[$attr])*]
            vis = $vis
            name = $name
            body = [$($body)*]
        }
    };
}

#[macro_export]
#[doc(hidden)]
macro_rules! define_mailbox_conversions {
    ($name:ident { $($var:ident ($ty:ty)),+ $(,)? }) => {
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

#[macro_export]
#[doc(hidden)]
macro_rules! define_messages_emit {
    (
        attrs = $attrs:tt
        vis = $vis:vis
        name = $name:ident
        body = [
            $(
                $(#[$var_attr:meta])*
                $var:ident
                $( { $($named:tt)* } )?
                $( ( $($tuple:tt)* ) )?
            ),+ $(,)?
        ]
    ) => {
        $(
            $crate::define_messages_struct! {
                $attrs
                $(#[$var_attr])*
                $vis $var $( { $($named)* } )? $( ( $($tuple)* ) )?
            }
        )+

        $crate::define_messages_apply! { $attrs
            $vis enum $name {
                $($var($var),)+
            }
        }

        $crate::define_mailbox_conversions!($name { $($var ($var)),+ });
    };
}

#[macro_export]
#[doc(hidden)]
macro_rules! define_messages_apply {
    ([$($attr:tt)*] $($item:tt)*) => {
        $($attr)*
        $($item)*
    };
}

#[macro_export]
#[doc(hidden)]
macro_rules! define_messages_copy_derives {
    ([] $($out:tt)*) => {
        $($out)*
    };
    ([#[doc $($inner:tt)*] $($rest:tt)*] $($out:tt)*) => {
        $crate::define_messages_copy_derives! { [$($rest)*] $($out)* }
    };
    ([#[$attr:meta] $($rest:tt)*] $($out:tt)*) => {
        $crate::define_messages_copy_derives! { [$($rest)*] #[$attr] $($out)* }
    };
}

#[macro_export]
#[doc(hidden)]
macro_rules! define_messages_struct {
    ($attrs:tt $(#[$var_attr:meta])* $vis:vis $name:ident { $($(#[$fattr:meta])* $field:ident : $ty:ty),* $(,)? }) => {
        $crate::define_messages_copy_derives! {
            $attrs
            $(#[$var_attr])*
            $vis struct $name {
                $($(#[$fattr])* pub $field: $ty,)*
            }
        }
    };
    ($attrs:tt $(#[$var_attr:meta])* $vis:vis $name:ident ( $($(#[$tattr:meta])* $ty:ty),* $(,)? )) => {
        $crate::define_messages_copy_derives! {
            $attrs
            $(#[$var_attr])*
            $vis struct $name($($(#[$tattr])* pub $ty,)*);
        }
    };
    ($attrs:tt $(#[$var_attr:meta])* $vis:vis $name:ident) => {
        $crate::define_messages_copy_derives! {
            $attrs
            $(#[$var_attr])*
            $vis struct $name;
        }
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
