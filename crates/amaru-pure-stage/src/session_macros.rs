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

//! Mermaid-like [`session_spec!`] syntax. Labels are type names (`stringify!`),
//! not dummy payload values.

/// Build a [`SessionSpec`](crate::session::SessionSpec) from a mermaid-like
/// state diagram. Message labels are identifiers (the `define_messages!` variant
/// structs). The automaton stores `stringify!` of those names; values are not
/// part of the spec.
///
/// States must implement [`State`](crate::typestate::State). Messages
/// must implement `Into<$enum>`. Agency (and optional timeout) is mermaid
/// `note left of` / `note right of`.
///
/// ```ignore
/// session_spec! {
///     Message;
///     [*] --> Idle
///     Idle --> Busy: RequestRange
///     Idle --> Done: ClientDone
///     Busy --> Idle: NoBlocks
///     Busy --> Streaming: StartBatch
///     Streaming --> Streaming: Block
///     Streaming --> Idle: BatchDone
///     note left of Idle: Initiator
///     note left of Busy: Responder timeout BLOCKFETCH_AGENCY_TIMEOUT
///     note left of Streaming: Responder timeout BLOCKFETCH_AGENCY_TIMEOUT
/// }
/// ```
#[macro_export]
macro_rules! session_spec {
    ($enum:ty; $($body:tt)*) => {
        $crate::__session_spec! { $enum, [], [], [], [], $($body)* }
    };
}

#[macro_export]
#[doc(hidden)]
macro_rules! __session_spec {
    ($enum:ty, $start:tt, $edges:tt, $notes:tt, $timeouts:tt,) => {
        $crate::__session_spec_emit! { $enum, $start, $edges, $notes, $timeouts }
    };

    ($enum:ty, $start:tt, $edges:tt, $notes:tt, $timeouts:tt, ) => {
        $crate::__session_spec_emit! { $enum, $start, $edges, $notes, $timeouts }
    };

    ($enum:ty, [$($start:ident)?], $edges:tt, $notes:tt, $timeouts:tt, [*] --> $to:ident $($rest:tt)*) => {
        $crate::__session_spec! { $enum, [$to], $edges, $notes, $timeouts, $($rest)* }
    };

    ($enum:ty, $start:tt, [$($edges:tt)*], $notes:tt, $timeouts:tt, $from:ident --> $to:ident : $msg:ident [sim_open] $($rest:tt)*) => {
        $crate::__session_spec! { $enum, $start, [$($edges)* ($from, $msg, $to, true)], $notes, $timeouts, $($rest)* }
    };

    ($enum:ty, $start:tt, [$($edges:tt)*], $notes:tt, $timeouts:tt, $from:ident --> $to:ident : $msg:ident $($rest:tt)*) => {
        $crate::__session_spec! { $enum, $start, [$($edges)* ($from, $msg, $to, false)], $notes, $timeouts, $($rest)* }
    };

    ($enum:ty, $start:tt, $edges:tt, [$($notes:tt)*], [$($timeouts:tt)*], note left of $s:ident : $role:ident timeout $dur:ident $($rest:tt)*) => {
        $crate::__session_spec! { $enum, $start, $edges, [$($notes)* ($s, $role)], [$($timeouts)* ($s, $dur)], $($rest)* }
    };

    ($enum:ty, $start:tt, $edges:tt, [$($notes:tt)*], [$($timeouts:tt)*], note right of $s:ident : $role:ident timeout $dur:ident $($rest:tt)*) => {
        $crate::__session_spec! { $enum, $start, $edges, [$($notes)* ($s, $role)], [$($timeouts)* ($s, $dur)], $($rest)* }
    };

    ($enum:ty, $start:tt, $edges:tt, [$($notes:tt)*], $timeouts:tt, note left of $s:ident : $role:ident $($rest:tt)*) => {
        $crate::__session_spec! { $enum, $start, $edges, [$($notes)* ($s, $role)], $timeouts, $($rest)* }
    };

    ($enum:ty, $start:tt, $edges:tt, [$($notes:tt)*], $timeouts:tt, note right of $s:ident : $role:ident $($rest:tt)*) => {
        $crate::__session_spec! { $enum, $start, $edges, [$($notes)* ($s, $role)], $timeouts, $($rest)* }
    };
}

#[macro_export]
#[doc(hidden)]
macro_rules! __session_spec_emit {
    (
        $enum:ty,
        [$($start:ident)?],
        [$(($from:ident, $msg:ident, $to:ident, $sim:expr))*],
        [$(($note_s:ident, $note_role:ident))*],
        [$(($ts:ident, $dur:expr))*]
    ) => {{
        {
            fn __state<S: $crate::typestate::State>() {}
            fn __wire<T: ::core::convert::Into<$enum>>() {}
            $(__state::<$start>();)?
            $(
                __state::<$from>();
                __state::<$to>();
                __wire::<$msg>();
            )*
            $(__state::<$note_s>();)*
            $(__state::<$ts>();)*
        }

        let mut spec = $crate::session::SessionSpec::default();
        $(spec.start(<$start as $crate::typestate::State>::NAME);)?

        let mut agency = ::std::collections::BTreeMap::<
            $crate::typestate::StateName,
            $crate::session::Agency,
        >::new();
        $(
            agency.insert(
                <$note_s as $crate::typestate::State>::NAME,
                $crate::session::Agency::$note_role,
            );
        )*

        $(
            {
                let from = <$from as $crate::typestate::State>::NAME;
                let to = <$to as $crate::typestate::State>::NAME;
                let msg = stringify!($msg);
                if $sim {
                    spec.sim_open(from, msg, to);
                } else {
                    match agency.get(from).copied() {
                        Some($crate::session::Agency::Initiator) => spec.init(from, msg, to),
                        Some($crate::session::Agency::Responder) => spec.resp(from, msg, to),
                        None => panic!(
                            "session_spec!: missing `note left of {from}: Initiator` or `Responder`"
                        ),
                    }
                }
            }
        )*

        $(
            spec.set_timeout(<$ts as $crate::typestate::State>::NAME, $dur);
        )*

        spec
    }};
}
