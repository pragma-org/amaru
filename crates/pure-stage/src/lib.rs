use either::Either;
use std::{future::Future, marker::PhantomData};

pub struct Zero;
pub struct Succ<T: Nat>(T);
pub trait Nat {}
impl Nat for Zero {}
impl<T: Nat> Nat for Succ<T> {}
pub const _0: Zero = Zero;
pub const _1: Succ<Zero> = Succ(_0);
pub const _2: Succ<Succ<Zero>> = Succ(_1);
pub const _3: Succ<Succ<Succ<Zero>>> = Succ(_2);

pub struct BiasedInput<T: List> {
    _marker: std::marker::PhantomData<T>,
}

pub trait InputExt<N: Nat> {
    type Output;
    fn read_at(&self, n: N) -> impl Future<Output = Self::Output>;
}

impl<T, U: List> InputExt<Zero> for BiasedInput<Either<T, U>> {
    type Output = T;
    fn read_at(&self, _n: Zero) -> impl Future<Output = T> {
        async { todo!() }
    }
}

impl<T, U, V: List, N: Nat> InputExt<Succ<N>> for BiasedInput<Either<T, Either<U, V>>>
where
    BiasedInput<Either<U, V>>: InputExt<N>,
{
    type Output = <BiasedInput<Either<U, V>> as InputExt<N>>::Output;
    fn read_at(&self, _n: Succ<N>) -> impl Future<Output = Self::Output> {
        async { todo!() }
    }
}

pub trait BiasedInputExt {
    type Output;
    fn read_biased(&self) -> impl Future<Output = Option<Self::Output>>;
}

impl<T> BiasedInputExt for BiasedInput<Just<T>> {
    type Output = T;
    fn read_biased(&self) -> impl Future<Output = Option<T>> {
        async { todo!() }
    }
}

impl<T, U: List> BiasedInputExt for BiasedInput<Either<T, U>>
where
    BiasedInput<U>: BiasedInputExt,
{
    type Output = Either<T, <BiasedInput<U> as BiasedInputExt>::Output>;

    fn read_biased(&self) -> impl Future<Output = Option<Self::Output>> {
        async { todo!() }
    }
}

pub struct Output<T: List> {
    _marker: std::marker::PhantomData<T>,
}

pub trait OutputExt<N: Nat> {
    type Input;
    fn send(&self, n: N, msg: Self::Input) -> impl Future<Output = ()>;
}

impl<T, U: List> OutputExt<Zero> for Output<Either<T, U>> {
    type Input = T;
    fn send(&self, _n: Zero, _msg: Self::Input) -> impl Future<Output = ()> {
        async { todo!() }
    }
}

impl<T, U: List, N: Nat> OutputExt<Succ<N>> for Output<Either<T, U>>
where
    Output<U>: OutputExt<N>,
{
    type Input = <Output<U> as OutputExt<N>>::Input;
    fn send(&self, _n: Succ<N>, _msg: Self::Input) -> impl Future<Output = ()> {
        async { todo!() }
    }
}

type Just<T> = Either<T, ()>;

pub trait List {}
impl List for () {}
impl<T, U: List> List for Either<T, U> {}

pub struct Stage<In, Out> {
    _ph: std::marker::PhantomData<(In, Out)>,
}

pub struct Network();

impl Network {
    pub fn new() -> Self {
        Self()
    }

    pub fn stage<In: List, Out: List, Aux>(
        &mut self,
        _f: impl std::ops::AsyncFn(BiasedInput<In>, Output<Out>, Aux) -> anyhow::Result<()>,
    ) -> Stage<In, Out> {
        todo!()
    }

    pub fn wire<LIn, LOut: List, L: Nat, RIn: List, ROut, R: Nat>(
        &mut self,
        _left: &Stage<LIn, LOut>,
        _left_out: L,
        _right: &Stage<RIn, ROut>,
        _right_in: R,
    ) -> Wire<<BiasedInput<RIn> as InputExt<R>>::Output>
    where
        BiasedInput<RIn>: InputExt<R>,
        Output<LOut>: OutputExt<L, Input = <BiasedInput<RIn> as InputExt<R>>::Output>,
    {
        todo!()
    }
}

pub struct Wire<T>(PhantomData<T>);

#[cfg(test)]
mod tests {
    use super::*;
    use either::Either;
    use std::sync::Arc;

    #[allow(dead_code)]
    #[test]
    fn it_works() {
        #[derive(Debug, Clone)]
        struct Point();
        #[derive(Debug, Clone)]
        struct Tip(Point, u32);
        struct Header();
        trait ChainStore<H> {}
        struct ClientState(Arc<dyn ChainStore<Header>>, Tip, Point);

        impl ClientState {
            fn new(store: Arc<dyn ChainStore<Header>>, tip: Tip, point: Point) -> Self {
                Self(store, tip, point)
            }
            pub fn next_op(&mut self) -> Option<BlockOp> {
                todo!()
            }
            pub fn add_op(&mut self, _op: BlockOp) {
                todo!()
            }
            pub fn tip(&self) -> Tip {
                todo!()
            }
        }

        fn find_headers_between(
            _store: &dyn ChainStore<Header>,
            _tip: &Tip,
            _points: &[Point],
        ) -> (Point, Tip) {
            todo!()
        }

        enum BlockOp {
            Forward(Point),
            Backward(Point),
        }

        #[derive(Debug)]
        enum ClientReq {
            Intersect(Vec<Point>),
            RequestNext,
        }

        enum ClientResponse {
            IntersectFound(Point, Tip),
            SendOp(BlockOp, Tip),
            Wait,
        }

        fn get_tip_from_point(_point: Point, _store: &dyn ChainStore<Header>) -> Tip {
            todo!()
        }

        async fn chain_sync(
            input: BiasedInput<Either<BlockOp, Just<ClientReq>>>,
            output: Output<Either<ClientResponse, Just<String>>>,
            store: Arc<dyn ChainStore<Header>>,
        ) -> anyhow::Result<()> {
            let mut tip = match input.read_at(_0).await {
                BlockOp::Forward(point, ..) | BlockOp::Backward(point) => {
                    get_tip_from_point(point, &*store)
                }
            };

            let mut state: ClientState = match input.read_at(_1).await {
                ClientReq::Intersect(points) => {
                    let (intersection, client_at) = find_headers_between(&*store, &tip, &points);
                    output
                        .send(
                            _0,
                            ClientResponse::IntersectFound(client_at.0.clone(), tip.clone()),
                        )
                        .await;
                    output.send(_1, "hello".to_owned()).await;
                    ClientState::new(store, client_at, intersection)
                }
                ClientReq::RequestNext => return Err(anyhow::anyhow!("RequestNext")),
            };

            let mut waiting = false;
            while let Some(msg) = input.read_biased().await {
                match msg {
                    Either::Left(op) => {
                        state.add_op(op);
                        tip = state.tip();
                        if waiting {
                            if let Some(op) = state.next_op() {
                                waiting = false;
                                output
                                    .send(_0, ClientResponse::SendOp(op, tip.clone()))
                                    .await;
                            }
                        }
                    }
                    Either::Right(ClientReq::RequestNext) => {
                        if let Some(op) = state.next_op() {
                            output
                                .send(_0, ClientResponse::SendOp(op, tip.clone()))
                                .await;
                        } else {
                            waiting = true;
                            output.send(_0, ClientResponse::Wait).await;
                        }
                    }
                    Either::Right(req) => return Err(anyhow::anyhow!("ClientReqError: {req:?}")),
                }
            }
            Ok(())
        }

        async fn dump_string(
            _input: BiasedInput<Just<String>>,
            _output: Output<()>,
            _aux: (),
        ) -> anyhow::Result<()> {
            todo!()
        }

        async fn dump_client_response(
            _input: BiasedInput<Just<ClientResponse>>,
            _output: Output<()>,
            _aux: (),
        ) -> anyhow::Result<()> {
            todo!()
        }

        let mut network = Network::new();

        let chain_sync = network.stage(chain_sync);
        let dump_string = network.stage(dump_string);
        let dump_client_response = network.stage(dump_client_response);

        let _: Wire<String> = network.wire(&chain_sync, _1, &dump_string, _0);
        let _: Wire<ClientResponse> = network.wire(&chain_sync, _0, &dump_client_response, _0);
    }
}
