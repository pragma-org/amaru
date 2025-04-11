# Pure Stage: Actor Model for deterministic scheduling

Design goals:

- no side effects, only explicit effects (which does include internal state changes)
- easy to use, including the ability to hold references between invocations
- executable on concurrent thread pool or using a deterministic simulator
- fully back-pressured
- ability for biased reading from inputs (which is necessary to avoid deadlocks with back-pressure)
- wiring code should be nicely readable

## Design elements

We will need to model state machines, processing network nodes, and wiring between those nodes.

### State Machines

Writing state machines in Rust can be done explicitly using an `enum` that implements a trait for computing `state × input → state × output`.
One advantage of this approach is that infrastructure like a (virtual) clock can be passed into the transition function as well.
The main disadvantage is that additional states are needed for modelling (biased) input operations, complicating the state machine code.

Another syntactic means to this end is an `async fn`, which gets translated by the compiler into a state machine like the one above, albeit with a limited transition function: the `Future::poll()` method.
When using this approach, effects like input and output need to be offered as `Future`s; the programmer may also await other Futures which perform untracked effects.
Such abuse can only be rejected using runtime checks because the return type of an `.await` point cannot be constrained.

### Processing Nodes

The main purpose of an API for declaring processing nodes is to obtain type-level information on the connectivity expected by this node, i.e. the number of inputs, outputs, and their respective message types.
Whether the state machine inside is implemented explicitly or using `async fn` doesn't matter at this level, we might offer both choices.

### Wiring

A wire is an object that connects one output to one input.
While Rust can in principle model type-state, this is inconvenient in practice because it requires shadowing and thus restricts the code a programmer can write.
Therefore, a compromise would be to establish that all processing nodes are declared first, followed by the declaration of the wires.
The wiring function ensures that message types do match, but it won't prevent connecting the same output to multiple inputs or multiple outputs to the same input; in fact, this freedom might be quite desirable.

## Sketch 1: Explicit State Machines

```rust
impl StateMachine for ChainSync {
    type InPorts = (ClientReq, BlockOp);
    type OutPorts = (ClientResp,);
    type StoreKey = Hash<28>;
    type StoreValue = Header;

    fn transition(self, input: Input<Self>, infra: &Infra<Self>) -> (Self, Effect<Self>) {
        match (self, input) {
            (Self::Initial(tip), Input::Bootstrap) => (Self::AwaitIntersect(tip), Effect::read_0()),
            (Self::AwaitIntersect(tip), Input::Received_0(ClientReq::Intersect(points))) => {
                let Some((intersection, client_at)) = find_headers_between(&*store.lock().await, &tip.0, &req)
                else {
                    return (Self::NoIntersection, Effect::write_0(ClientResp::IntersectNotFound));
                };
                let mut state = ClientState::new(store, intersection.into(), client_at.clone());
                (Self::OpenForBusiness(tip.clone(), state, false), Effect::write_0(ClientResp::IntersectFound(client_at.0, tip)))
            }
            (Self::NoIntersection, Input::Written_0) => (Self::ProtocolFailed, Effect::Terminate),
            (s @ Self::OpenForBusiness(..), Input::Written_0) => {
                (s, Effect::read_biased((Effect::read_1(), Effect::read_0())))
            }
            (Self::OpenForBusiness(mut tip, mut state, waiting), Input::Read_1(op)) => {
                state.add_op[op, &mut tip]; // FIXME rollback requires store to find full Tip
                if waiting {
                    (Self::GetNextOp(tip, state), Effect::ReadStore)
                } else {
                    (Self::OpenForBusiness(tip, state, waiting), Effect::read_biased((Effect::read_1(), Effect::read_0())))
                }
            }
            (Self::OpenForBusiness(tip, state, _), Input::Read_0(clientReq::RequestNext)) => {
                (Self::GetNextOp(tip, state), Effect::ReadStore)
            }
            (Self::GetNextOp(tip, state), Input::Store(store)) => {
                if let Some((op, tip)) = state.next_ip(store) {
                    (Self::OpenForBusiness(tip, state, false), Effect::write_0(ClientResp::Next(op, tip)))
                } else {
                    (Self::OpenForBusiness(tip, state, true), Effect::Idle)
                }
            }
        }
    }
}
```

Findings:

- quite complicated
- was difficult to think up and probably still as bugs I didn’t realise
- state progression is hard to see when reading the source code

## Sketch 2: effectful function from inputs to outputs

```rust
async fn chain_sync(state: ChainSync, input: ChainSyncMsg) -> (ChainSync, Vec<ChainSyncOutput>) {
    match (state, input) {
        (ChainSync::Initial(tip), ChainSyncMsg::BlockOp(op)) => 
        // mostly like the above, but with less intermediate states e.g. for reading the store or for output
    }
}
```

## Sketch 3: effectful function reading inputs explicitly

```rust
async fn chain_sync(input: BiasedInput<Either<BlockOp, ClientReq>>, output: Output<ClientResponse>, store: Arc<dyn Store>) -> anyhow::Result<()> {
    let mut tip = match input.read(0).await {
        BlockOp::Forward(point, ..) | BlockOp::Backward(point) => {
            get_tip_from_point(point, &store)
        }
    };

    let mut state = match input.read(1).await {
        ClientReq::Intersect(points) => {
            let (intersection, client_at) = find_headers_between(&store, &tip.0, &points);
            output.send(ClientResponse::IntersectFound(client_at.0.clone(), tip.clone())).await;
            ClientState::new(store, intersection.into(), client_at)
        }
    }

    let mut waiting = false;
    while let Some(msg) = input.read_biased().await {
        match msg {
            Either::Left(BlockOp::Forward(..)) => todo!(),
            Either::Left(BlockOp::Backward(..)) => todo!(),
            Either::Right(ClientReq::RequestNext) => todo!(),
            Either::Right(req) => return Err(ClientReqError(req).into())
        }
    }
}
```
