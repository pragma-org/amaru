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

use std::{any::Any, fmt, marker::PhantomData, ops::Deref, sync::Arc};

use crate::{BLACKHOLE_NAME, Name, SendData};

/// A handle to a stage during the building phase of a [`StageGraph`](crate::StageGraph).
pub struct StageBuildRef<Msg, St, RefAux> {
    pub name: Name,
    pub(crate) network: RefAux,
    pub(crate) _ph: PhantomData<(Msg, St)>,
}

impl<Msg, State, RefAux> StageBuildRef<Msg, State, RefAux> {
    /// Derive the handle that can later be used for sending messages to this stage.
    pub fn sender(&self) -> StageRef<Msg> {
        StageRef { name: self.name.clone(), extra: None, _ph: PhantomData }
    }
}

/// Injection applied at send/call time so a [`StageRef`] can accept a different message type
/// than the destination stage.
///
/// `next` is only a leftover call-reply payload (`StageRefExtra` / `ScheduleId`), never another
/// [`Injection`]: [`StageRef::contramap`] composes transforms into a single function.
pub(crate) struct Injection {
    pub transform: Arc<dyn Fn(Box<dyn SendData>) -> Box<dyn SendData> + Send + Sync>,
    pub next: Option<Arc<dyn Any + Send + Sync>>,
}

impl Injection {
    fn from_extra(extra: &Option<Arc<dyn Any + Send + Sync>>) -> Option<&Self> {
        extra.as_ref().and_then(|extra| extra.downcast_ref::<Self>())
    }
}

/// A handle for sending messages to a stage via the [`Effects`](crate::Effects) argument to the stage transition function.
///
/// [`Self::contramap`] stores a type-injection on this handle. The name remains the original
/// stage name; the transform is not serialized (`extra` is skipped).
#[derive(serde::Serialize, serde::Deserialize)]
pub struct StageRef<Msg> {
    name: Name,
    #[serde(skip)]
    extra: Option<Arc<dyn Any + Send + Sync>>,
    #[serde(skip)]
    _ph: PhantomData<Msg>,
}

impl<Msg> PartialEq for StageRef<Msg> {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl<Msg> Eq for StageRef<Msg> {}

impl<Msg> Clone for StageRef<Msg> {
    fn clone(&self) -> Self {
        Self { name: self.name.clone(), extra: self.extra.clone(), _ph: PhantomData }
    }
}

impl<Msg> fmt::Debug for StageRef<Msg> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "StageRef({})", self.name)
    }
}

impl<Msg> AsRef<StageRef<Msg>> for StageRef<Msg> {
    fn as_ref(&self) -> &StageRef<Msg> {
        self
    }
}

impl<Msg> AsRef<Name> for StageRef<Msg> {
    fn as_ref(&self) -> &Name {
        &self.name
    }
}

impl<Msg> AsRef<str> for StageRef<Msg> {
    fn as_ref(&self) -> &str {
        self.name.as_str()
    }
}

impl<Msg> StageRef<Msg> {
    pub(crate) fn new(name: Name) -> Self {
        Self { name, extra: None, _ph: PhantomData }
    }

    pub(crate) fn with_extra(self, extra: Arc<dyn Any + Send + Sync>) -> Self {
        Self { extra: Some(extra), ..self }
    }

    pub fn named_for_tests(name: &str) -> StageRef<Msg> {
        StageRef::new(Name::from(name))
    }

    pub fn blackhole() -> StageRef<Msg> {
        StageRef::new(BLACKHOLE_NAME.clone())
    }

    pub fn name(&self) -> &Name {
        &self.name
    }

    pub fn is_blackhole(&self) -> bool {
        self.name == *BLACKHOLE_NAME
    }

    pub(crate) fn extra(&self) -> Option<&Arc<dyn Any + Send + Sync>> {
        self.extra.as_ref()
    }

    /// View this stage as accepting `Mapped` messages, injecting them into this ref's
    /// message type at send time.
    ///
    /// The returned ref keeps [`name`](Self::name) of `self`. No runtime name is allocated and
    /// dropping the ref cannot leak. The trace buffer records the already-injected message
    /// sent to that original name.
    ///
    /// The transform is held on the live handle (`extra`) and is skipped by serde. Replaying a
    /// recorded [`Send`](crate::Effect::Send) is fine (the payload is already injected);
    /// reconstructing this ref from a snapshot and sending again will not apply the injection.
    pub fn contramap<Mapped: SendData>(
        &self,
        transform: impl Fn(Mapped) -> Msg + Send + Sync + 'static,
    ) -> StageRef<Mapped>
    where
        Msg: SendData,
    {
        let (prev, leftover) = match Injection::from_extra(&self.extra) {
            Some(inj) => (Some(inj.transform.clone()), inj.next.clone()),
            None => (None, self.extra.clone()),
        };
        let transform = Arc::new(move |boxed: Box<dyn SendData>| {
            #[expect(clippy::expect_used)]
            let mapped = boxed.cast::<Mapped>().expect("internal message type error");
            let original = transform(*mapped);
            match &prev {
                Some(prev) => prev(Box::new(original)),
                None => Box::new(original) as Box<dyn SendData>,
            }
        });
        StageRef {
            name: self.name.clone(),
            extra: Some(Arc::new(Injection { transform, next: leftover })),
            _ph: PhantomData,
        }
    }

    /// Split off leftover call-reply extra and the composed injection, if any.
    pub(crate) fn peel(&self) -> Peeled {
        match Injection::from_extra(&self.extra) {
            Some(inj) => {
                Peeled { name: self.name.clone(), leftover: inj.next.clone(), transform: Some(inj.transform.clone()) }
            }
            None => Peeled { name: self.name.clone(), leftover: self.extra.clone(), transform: None },
        }
    }

    /// Apply any [`contramap`](Self::contramap) injections and split off a leftover call-reply extra.
    pub(crate) fn materialize_send(&self, msg: Msg) -> (Name, Option<Arc<dyn Any + Send + Sync>>, Box<dyn SendData>)
    where
        Msg: SendData,
    {
        let peeled = self.peel();
        let payload = match peeled.transform {
            Some(transform) => transform(Box::new(msg)),
            None => Box::new(msg),
        };
        (peeled.name, peeled.leftover, payload)
    }
}

pub(crate) struct Peeled {
    pub name: Name,
    pub leftover: Option<Arc<dyn Any + Send + Sync>>,
    pub transform: Option<Arc<dyn Fn(Box<dyn SendData>) -> Box<dyn SendData> + Send + Sync>>,
}

/// A handle for sending messages to a stage via the [`Effects`](crate::Effects) argument to the stage transition function.
///
/// This is a variant that is mostly useful in tests because it allows extracting the current state of the stage.
#[derive(PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StageStateRef<Msg, St> {
    stage_ref: StageRef<Msg>,
    #[serde(skip)]
    pub(crate) _ph: PhantomData<St>,
}

impl<Msg, St> Clone for StageStateRef<Msg, St> {
    fn clone(&self) -> Self {
        Self { stage_ref: self.stage_ref.clone(), _ph: self._ph }
    }
}

impl<Msg, St> StageStateRef<Msg, St> {
    pub(crate) fn new(name: Name) -> Self {
        Self { stage_ref: StageRef::new(name), _ph: PhantomData }
    }
}

impl<Msg, St> fmt::Debug for StageStateRef<Msg, St> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.stage_ref.fmt(f)
    }
}

impl<Msg, St> StageStateRef<Msg, St> {
    pub fn without_state(self) -> StageRef<Msg> {
        self.stage_ref
    }
}

impl<Msg, St> Deref for StageStateRef<Msg, St> {
    type Target = StageRef<Msg>;

    fn deref(&self) -> &Self::Target {
        &self.stage_ref
    }
}

impl<Msg, St> AsRef<StageRef<Msg>> for StageStateRef<Msg, St> {
    fn as_ref(&self) -> &StageRef<Msg> {
        &self.stage_ref
    }
}

#[test]
fn stage_ref() {
    let stage = StageRef { name: "test".into(), extra: None, _ph: PhantomData::<(u32, u64)> };

    fn send<T: Send>(_t: &T) {}
    fn sync<T: Sync>(_t: &T) {}

    send(&stage);
    sync(&stage);
}

#[test]
fn contramap_keeps_original_name() {
    let stage = StageRef::<u32>::named_for_tests("dest");
    let mapped = stage.contramap(|x: u8| u32::from(x));
    assert_eq!(mapped.name().as_str(), "dest");
    assert_eq!(mapped.name(), stage.name());
}

#[test]
fn contramap_applies_and_composes() {
    let stage = StageRef::<u32>::named_for_tests("dest");
    let once = stage.contramap(|x: u16| u32::from(x) + 1);
    let twice = once.contramap(|x: u8| u16::from(x) * 2);
    assert_eq!(twice.name().as_str(), "dest");

    let (name, leftover, payload) = twice.materialize_send(3);
    assert_eq!(name.as_str(), "dest");
    assert!(leftover.is_none());
    assert_eq!(*payload.cast::<u32>().unwrap(), 7); // 3 * 2 + 1
}

#[test]
fn contramap_preserves_call_extra() {
    let extra: Arc<dyn Any + Send + Sync> = Arc::new(7u64);
    let stage = StageRef::<u32>::new("dest".into()).with_extra(extra);
    let mapped = stage.contramap(|x: u8| u32::from(x));
    let (_name, leftover, payload) = mapped.materialize_send(1);
    assert_eq!(*payload.cast::<u32>().unwrap(), 1);
    assert_eq!(leftover.and_then(|e| e.downcast_ref::<u64>().copied()), Some(7));
}
