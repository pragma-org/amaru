use crate::{
    effect::{StageEffect, StageResponse},
    simulation::EffectBox,
    BoxFuture, Effects, Instant, Message, Name, Sender, StageBuildRef, StageGraph, StageRef, State,
};
use either::Either::{Left, Right};
use parking_lot::Mutex;
use std::{
    collections::BTreeMap,
    future::Future,
    marker::PhantomData,
    sync::Arc,
    task::{Context, Poll, Waker},
};
use tokio::{
    runtime::Handle,
    sync::mpsc::{self, Receiver},
    task::JoinHandle,
};

#[derive(Debug, thiserror::Error)]
#[error("message send failed to stage `{target}`")]
pub struct SendError {
    target: Name,
}

struct TokioInner {
    senders: BTreeMap<Name, mpsc::Sender<Box<dyn Message>>>,
    now: Arc<dyn Fn() -> Instant + Send + Sync>,
    mailbox_size: usize,
}

impl Default for TokioInner {
    fn default() -> Self {
        Self {
            senders: Default::default(),
            now: Arc::new(|| Instant::from_tokio(tokio::time::Instant::now())),
            mailbox_size: 10,
        }
    }
}

/// A [`StageGraph`] implementation that dispatches each stage as a task on the Tokio global pool.
///
/// *This is currently only a minimal sketch that will likely not fit the intended design.
/// It is more likely that the effect handling will be done like in the [`SimulationBuilder`](crate::simulation::SimulationBuilder)
/// implementation.*
#[derive(Default)]
pub struct TokioBuilder {
    tasks: Vec<Box<dyn FnOnce(Arc<TokioInner>) -> BoxFuture<'static, anyhow::Result<()>>>>,
    inner: TokioInner,
}

impl StageGraph for TokioBuilder {
    type Running = TokioRunning;

    type RefAux<Msg, State> = (
        Receiver<Box<dyn Message>>,
        Box<
            dyn FnMut(State, Msg, Effects<Msg, State>) -> BoxFuture<'static, anyhow::Result<State>>
                + 'static
                + Send,
        >,
    );

    fn stage<Msg: Message, St: State, F, Fut>(
        &mut self,
        name: impl AsRef<str>,
        mut f: F,
    ) -> StageBuildRef<Msg, St, Self::RefAux<Msg, St>>
    where
        F: FnMut(St, Msg, Effects<Msg, St>) -> Fut + 'static + Send,
        Fut: Future<Output = anyhow::Result<St>> + 'static + Send,
    {
        // THIS MUST MATCH THE SIMULATION BUILDER
        let name = Name::from(&*format!("{}-{}", name.as_ref(), self.inner.senders.len()));
        let (tx, rx) = mpsc::channel(self.inner.mailbox_size);
        self.inner.senders.insert(name.clone(), tx);
        StageBuildRef {
            name,
            network: (
                rx,
                Box::new(move |state, msg, eff| Box::pin(f(state, msg, eff))),
            ),
            _ph: PhantomData,
        }
    }

    #[allow(clippy::expect_used)]
    fn wire_up<Msg: Message, St: State>(
        &mut self,
        stage: StageBuildRef<Msg, St, Self::RefAux<Msg, St>>,
        mut state: St,
    ) -> StageRef<Msg, St> {
        let StageBuildRef {
            name,
            network: (mut rx, mut ff),
            _ph,
        } = stage;
        let stage_name = name.clone();
        self.tasks.push(Box::new(move |inner| {
            Box::pin(async move {
                let me = StageRef {
                    name: stage_name.clone(),
                    _ph: PhantomData,
                };
                let effect = Arc::new(Mutex::new(None));
                let sender = mk_sender(&stage_name, &inner);
                let effects = Effects::new(me, effect.clone(), inner.now.clone(), sender);
                while let Some(msg) = rx.recv().await {
                    state = interpreter(
                        &inner,
                        &effect,
                        &stage_name,
                        ff(
                            state,
                            *msg.cast::<Msg>().expect("internal message type error"),
                            effects.clone(),
                        ),
                    )
                    .await
                    .inspect_err(|err| {
                        tracing::error!("stage `{}` error: {:?}", stage_name, err);
                    })?;
                }
                Ok(())
            })
        }));
        StageRef {
            name,
            _ph: PhantomData,
        }
    }

    #[allow(clippy::expect_used)]
    fn sender<Msg: Message, St>(&mut self, stage: &StageRef<Msg, St>) -> Sender<Msg> {
        mk_sender(&stage.name, &self.inner)
    }

    fn run(self, rt: Handle) -> Self::Running {
        let Self { tasks, inner } = self;
        let inner = Arc::new(inner);
        let handles = tasks
            .into_iter()
            .map(|t| rt.spawn(t(inner.clone())))
            .collect();
        TokioRunning { handles }
    }
}

#[allow(clippy::expect_used)]
fn mk_sender<Msg: Message>(stage_name: &Name, inner: &TokioInner) -> Sender<Msg> {
    let tx = inner
        .senders
        .get(stage_name)
        .expect("stage ref contained unknown name")
        .clone();
    let sender = Sender::new(Arc::new(move |msg: Msg| {
        let tx = tx.clone();
        Box::pin(async move {
            tx.send(Box::new(msg))
                .await
                .map_err(|msg| *msg.0.cast::<Msg>().expect("message was just boxed"))
        })
    }));
    sender
}

async fn interpreter<St>(
    inner: &TokioInner,
    effect: &EffectBox,
    name: &Name,
    mut stage: BoxFuture<'static, anyhow::Result<St>>,
) -> anyhow::Result<St> {
    loop {
        let poll = stage.as_mut().poll(&mut Context::from_waker(Waker::noop()));
        if let Poll::Ready(state) = poll {
            return state;
        }
        #[allow(clippy::panic)]
        let Some(Left(eff)) = effect.lock().take() else {
            panic!("stage `{name}` used .await on something that was not a stage effect");
        };
        let resp = match eff {
            StageEffect::Receive => {
                #[allow(clippy::panic)]
                {
                    panic!("effect Receive cannot be explicitly awaited (stage `{name}`)")
                }
            }
            StageEffect::Send(name, msg, call) => {
                #[allow(clippy::expect_used)]
                let tx = inner
                    .senders
                    .get(&name)
                    .expect("stage ref contained unknown name");
                tx.send(msg).await.map_err(|_| SendError {
                    target: name.clone(),
                })?;
                if let Some((d, rx, _id)) = call {
                    tokio::time::timeout(d, rx)
                        .await
                        .ok()
                        .and_then(|r| r.ok())
                        .map(StageResponse::CallResponse)
                        .unwrap_or(StageResponse::CallTimeout)
                } else {
                    StageResponse::Unit
                }
            }
            StageEffect::Clock => StageResponse::ClockResponse(now()),
            StageEffect::Wait(duration) => {
                tokio::time::sleep(duration).await;
                StageResponse::WaitResponse(now())
            }
            StageEffect::Call(..) => {
                #[allow(clippy::panic)]
                {
                    panic!("StageEffect::Call cannot be explicitly awaited (stage `{name}`")
                }
            }
            StageEffect::Respond(target, _call_id, deadline, sender, msg) => {
                if let Err(msg) = sender.send(msg) {
                    tracing::warn!(
                        "response to {} was dropped: {:?} (deadline: {})",
                        target,
                        msg,
                        deadline.pretty(now())
                    );
                }
                StageResponse::Unit
            }
            StageEffect::External(effect) => {
                tracing::debug!("stage `{name}` external effect: {:?}", effect);
                StageResponse::ExternalResponse(effect.run().await)
            }
        };
        *effect.lock() = Some(Right(resp));
    }
}

fn now() -> Instant {
    Instant::from_tokio(tokio::time::Instant::now())
}

/// Handle to the running stages.
#[must_use = "this handle needs to be either joined or aborted"]
pub struct TokioRunning {
    handles: Vec<JoinHandle<anyhow::Result<()>>>,
}

impl TokioRunning {
    /// Abort all stage tasks of this network.
    pub fn abort(self) {
        for handle in self.handles {
            handle.abort();
        }
    }

    pub async fn join(self) -> Vec<anyhow::Result<()>> {
        let mut res = Vec::new();
        for handle in self.handles.into_iter() {
            res.push(handle.await.unwrap_or_else(|err| Err(err.into())));
        }
        res
    }
}
