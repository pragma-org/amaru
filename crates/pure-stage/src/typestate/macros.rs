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

#[macro_export]
macro_rules! make_states {
    ($($init:ident),+; $($other:ident),+) => {
        $(
            pub struct $init($crate::typestate::Marker);
            impl $crate::typestate::State for $init {
                const NAME: &'static str = stringify!($init);
                const MAKE: fn($crate::typestate::Marker) -> Self = $init;
                type Initial = $crate::typestate::InitialState;
            }
        )*
        $(
            pub struct $other($crate::typestate::Marker);
            impl $crate::typestate::State for $other {
                const NAME: &'static str = stringify!($other);
                const MAKE: fn($crate::typestate::Marker) -> Self = $other;
                type Initial = $crate::typestate::NotInitialState;
            }
        )*
    };
}

#[macro_export]
macro_rules! effects {
    ($($t:ty),*) => {
        $crate::typestate::Cons<effects!(@ $($t),*), $crate::typestate::Nil>
    };
    ($($t:ty),* | $($($eff:ty),*)|*) => {
        $crate::typestate::Cons<effects!(@ $($t),*), effects!($($($eff),*)|*)>
    };
    (@ $t:ty) => {
        $crate::typestate::Cons<$t, $crate::typestate::Nil>
    };
    (@ $t:ty, $($eff:ty),*) => {
        $crate::typestate::Cons<$t, effects!(@ $($eff),*)>
    };
}

#[macro_export]
macro_rules! transition {
    ($from:ty => $to:ty: $eff:ty) => {
        impl $crate::typestate::Transition<$to> for $from {
            type Eff = $eff;
        }
    };
}
