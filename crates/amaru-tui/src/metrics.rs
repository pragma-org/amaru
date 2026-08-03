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

use std::{
    sync::{Arc, mpsc::SyncSender},
    time::Instant,
};

use amaru_metrics::{MetricsEvent, subscribe};

use crate::events::{Message, MetricRecord};

#[derive(Debug)]
pub struct Subscriber {
    tx: SyncSender<Message>,
}

impl Subscriber {
    pub fn new(tx: SyncSender<Message>) -> Self {
        Self { tx }
    }

    pub fn emit(&self, event: &MetricsEvent) {
        let _ = self.tx.try_send(Message::Metrics(MetricRecord { at: Instant::now(), event: event.clone() }));
    }
}

#[derive(Debug)]
pub struct Subscription {
    _inner: amaru_metrics::Subscription,
}

impl Subscription {
    pub fn new(subscriber: Arc<Subscriber>) -> Self {
        let inner = subscribe(Arc::new(move |event| subscriber.emit(event)));
        Self { _inner: inner }
    }
}
