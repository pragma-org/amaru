// Copyright 2025 PRAGMA
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

use futures_util::Stream;
use std::{
    pin::Pin,
    task::{Context, Poll},
};
use tokio::sync::mpsc;

/// The message receptacle used by [`StageGraph::output`](crate::StageGraph::output).
///
/// It should be noted that [`Self::try_next`] returning `None` only means that the message
/// queue is currently empty — it may be refilled by future simulation steps.
#[derive(Debug)]
pub struct Receiver<T> {
    rx: mpsc::Receiver<T>,
}

impl<T> Receiver<T> {
    pub fn new(rx: mpsc::Receiver<T>) -> Self {
        Self { rx }
    }

    /// Extract the next message if there is one.
    pub fn try_next(&mut self) -> Option<T> {
        self.rx.try_recv().ok()
    }

    /// Produce an iterator over all messages currently enqueued.
    pub fn drain(&mut self) -> impl Iterator<Item = T> + '_ {
        struct Iter<'a, T>(&'a mut Receiver<T>);
        impl<'a, T> Iterator for Iter<'a, T> {
            type Item = T;

            fn next(&mut self) -> Option<Self::Item> {
                self.0.try_next()
            }
        }
        Iter(self)
    }
}

impl<T> Stream for Receiver<T> {
    type Item = T;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.rx.poll_recv(cx)
    }
}
