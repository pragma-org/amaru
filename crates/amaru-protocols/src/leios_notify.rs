use amaru_kernel::cbor;
use pure_stage::{StageRef, Void};

use crate::{
    mux::MuxMessage,
    protocol::{
        Initiator, Inputs, Miniprotocol, Outcome, PROTO_N2N_LEIOS_NOTIFY, ProtocolState, StageState, miniprotocol,
        outcome,
    },
};

pub fn initiator() -> Miniprotocol<LeiosNotifyProto, LeiosNotify, Initiator> {
    miniprotocol(PROTO_N2N_LEIOS_NOTIFY)
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LeiosNotify {
    muxer: StageRef<MuxMessage>,
}

impl LeiosNotify {
    pub fn new(muxer: StageRef<MuxMessage>) -> Self {
        Self { muxer }
    }
}

impl StageState<LeiosNotifyProto, Initiator> for LeiosNotify {
    type LocalIn = ();

    async fn local(
        self,
        _proto: &LeiosNotifyProto,
        _input: Self::LocalIn,
        _eff: &pure_stage::Effects<Inputs<Self::LocalIn>>,
    ) -> anyhow::Result<(Option<()>, Self)> {
        todo!()
    }

    async fn network(
        self,
        _proto: &LeiosNotifyProto,
        input: Vec<u8>,
        _eff: &pure_stage::Effects<Inputs<Self::LocalIn>>,
    ) -> anyhow::Result<(Option<()>, Self)> {
        let msg = cbor_data::Cbor::checked(&input).unwrap();
        tracing::info!(data = %msg, "received data from leios");
        Ok((Some(()), self))
    }

    fn muxer(&self) -> &pure_stage::StageRef<crate::mux::MuxMessage> {
        &self.muxer
    }
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LeiosNotifyProto;

impl ProtocolState<Initiator> for LeiosNotifyProto {
    type WireMsg = LeiosNotifyMsg;

    type Action = ();

    type Out = Vec<u8>;

    type Error = Void;

    fn init(&self) -> anyhow::Result<(Outcome<Self::WireMsg, Self::Out, Self::Error>, Self)> {
        tracing::info!("initializing leios notify");
        Ok((outcome().send(LeiosNotifyMsg::RequestNext).want_next(), *self))
    }

    fn network(&self, input: Self::WireMsg) -> anyhow::Result<(Outcome<Self::WireMsg, Self::Out, Self::Error>, Self)> {
        tracing::info!("network leios notify");
        let LeiosNotifyMsg::Other(data) = input else {
            anyhow::bail!("invalid message");
        };
        Ok((outcome().result(data).want_next(), *self))
    }

    fn local(
        &self,
        _input: Self::Action,
    ) -> anyhow::Result<(Outcome<Self::WireMsg, pure_stage::Void, Self::Error>, Self)> {
        tracing::info!("local leios notify");
        Ok((outcome().send(LeiosNotifyMsg::RequestNext), *self))
    }
}

pub enum LeiosNotifyMsg {
    RequestNext,
    Other(Vec<u8>),
}

impl cbor::Encode<()> for LeiosNotifyMsg {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        _: &mut (),
    ) -> Result<(), minicbor::encode::Error<W::Error>> {
        match self {
            LeiosNotifyMsg::RequestNext => {
                e.array(1)?.u16(0)?;
            }
            LeiosNotifyMsg::Other(_data) => {
                unimplemented!();
            }
        }
        Ok(())
    }
}

impl<'b> cbor::Decode<'b, ()> for LeiosNotifyMsg {
    fn decode(d: &mut cbor::Decoder<'b>, _: &mut ()) -> Result<Self, cbor::decode::Error> {
        Ok(Self::Other(d.input().to_vec()))
    }
}
