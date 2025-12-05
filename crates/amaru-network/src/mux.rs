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

use crate::{
    bytes::DebugBytes,
    effects::{Network, NetworkOps},
    protocol::{Erased, ProtocolId, Role},
    socket::ConnectionId,
};
use anyhow::Context;
use bytes::{Buf, BufMut, Bytes, BytesMut};
use cbor_data::{Cbor, ErrorKind, ParseError};
use pure_stage::{CallRef, Effects, StageRef, TryInStage};
#[expect(clippy::disallowed_types)]
use std::collections::HashMap;
use std::{
    cell::RefCell,
    collections::{VecDeque, hash_map::Entry},
    time::SystemTime,
};
use tracing::{Level, instrument};

const MAX_SEGMENT_SIZE: usize = 65535;

/// microseconds part of the wall clock time
#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Timestamp(u32);

impl Timestamp {
    fn now() -> Self {
        #[expect(clippy::expect_used)]
        Self(
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .expect("system time is not supposed to be before the UNIX epoch")
                .as_micros() as u32,
        )
    }

    fn encode(self, buffer: &mut BytesMut) {
        buffer.put_u32(self.0);
    }

    fn decode(buffer: &mut Bytes) -> Self {
        Self(buffer.get_u32())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Frame {
    /// Each message is a single CBOR item
    OneCborItem,
    /// No message parsing, just buffer the data
    Buffer,
}

impl Frame {
    pub fn try_consume(&self, data: &mut BytesMut) -> Result<Option<Bytes>, ParseError> {
        match self {
            Frame::OneCborItem => match Cbor::checked_prefix(data) {
                Ok((item, _rest)) => {
                    let item = data.copy_to_bytes(item.as_slice().len());
                    Ok(Some(item))
                }
                Err(e) if matches!(e.kind(), ErrorKind::UnexpectedEof(_)) => Ok(None),
                Err(e) => Err(e),
            },
            Frame::Buffer => Ok(None),
        }
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Sent;

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Read;

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum MuxMessage {
    /// Register the given protocol with its ID so that data will be fed into it
    ///
    /// The protocol will get the first invocation for free and must then request
    /// following invocations by sending `WantNext` (for strict flow control).
    Register {
        protocol: ProtocolId<Erased>,
        frame: Frame,
        handler: StageRef<Bytes>,
        max_buffer: usize,
    },
    /// Buffer incoming data for this protocol ID up to the given limit
    /// (this should be followed by Register eventually, to then consume the data)
    ///
    /// Setting the size to zero means that data are dropped without begin buffered
    /// and without tearing down the connection.
    Buffer(ProtocolId<Erased>, usize),
    /// Send the given message on the protocol ID and notify when enqueued in TCP buffer
    Send(ProtocolId<Erased>, DebugBytes, CallRef<Sent>),
    /// internal message coming from the TCP stream reader
    FromNetwork(Timestamp, ProtocolId<Erased>, DebugBytes),
    /// Notify that the segment has been written to the TCP stream
    Written,
    /// Permit the next invocation of the Protocol with data from the network.
    WantNext(ProtocolId<Erased>),
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct State {
    conn: Connection,
    muxer: Muxer,
    sending: bool,
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
enum Connection {
    Unint(ConnectionId),
    Init(StageRef<DebugBytes>, StageRef<Read>),
}

impl State {
    /// Create a new state with the given connection ID and buffering the given protocols.
    ///
    /// Note that upon receiving the first message, the stage will start reading from the network.
    /// Any data received for unregistered protocols will lead to stage termination.
    pub fn new(conn: ConnectionId, buffer: &[(ProtocolId<Erased>, usize)]) -> Self {
        let mut muxer = Muxer::new();
        for &(proto_id, limit) in buffer {
            #[expect(clippy::expect_used)]
            muxer.buffer(proto_id, limit).expect("no buffered data yet");
        }
        Self {
            conn: Connection::Unint(conn),
            muxer,
            sending: false,
        }
    }

    pub async fn init(
        &mut self,
        eff: &mut Effects<MuxMessage>,
    ) -> (
        &mut Muxer,
        &mut bool,
        &StageRef<DebugBytes>,
        &StageRef<Read>,
    ) {
        match &mut self.conn {
            Connection::Unint(conn) => {
                let writer = eff
                    .stage(
                        format!("writer-{}", conn),
                        |(conn, muxer), data: DebugBytes, eff| async move {
                            Network::new(&eff)
                            .send(conn, data)
                            .await
                            .or_terminate(
                                &eff,
                                async |err| tracing::error!(%err, "failed to send data to network"),
                            )
                            .await;
                            eff.send(&muxer, MuxMessage::Written).await;
                            (conn, muxer)
                        },
                    )
                    .await;
                let writer = eff.wire_up(writer, (*conn, eff.me())).await;
                let reader = eff.stage(format!("reader-{}", conn), read_segment).await;
                let reader = eff.wire_up(reader, (*conn, eff.me())).await;
                eff.send(&reader, Read).await;
                self.conn = Connection::Init(writer, reader);
            }
            Connection::Init(..) => {}
        }
        let Connection::Init(writer, reader) = &self.conn else {
            unreachable!()
        };
        (&mut self.muxer, &mut self.sending, writer, reader)
    }
}

pub async fn stage(mut state: State, msg: MuxMessage, mut eff: Effects<MuxMessage>) -> State {
    let (muxer, sending, writer, reader) = state.init(&mut eff).await;

    handle_msg(msg, &eff, muxer, sending, writer, reader)
        .await
        .or_terminate(&eff, async |err| tracing::error!(%err, "muxing error"))
        .await;

    state
}

async fn handle_msg(
    msg: MuxMessage,
    eff: &Effects<MuxMessage>,
    muxer: &mut Muxer,
    sending: &mut bool,
    writer: &StageRef<DebugBytes>,
    reader: &StageRef<Read>,
) -> anyhow::Result<()> {
    match msg {
        MuxMessage::Register {
            protocol,
            frame,
            handler,
            max_buffer,
        } => {
            muxer
                .register(protocol, frame, max_buffer, handler, eff)
                .await
        }
        MuxMessage::Buffer(proto_id, limit) => muxer.buffer(proto_id, limit),
        MuxMessage::Send(proto_id, bytes, sent) => {
            debug_assert!(
                !bytes.is_empty(),
                "sending empty message for protocol {} is forbidden",
                proto_id
            );
            tracing::trace!(%proto_id, bytes = bytes.len(), "send");
            muxer.outgoing(proto_id, bytes.into(), sent);
            if !*sending && let Some((proto_id, bytes)) = muxer.next_segment(eff).await {
                *sending = true;
                eff.send(writer, Header::encode(proto_id, &bytes).into())
                    .await;
            }
            Ok(())
        }
        MuxMessage::FromNetwork(timestamp, proto_id, bytes) => {
            tracing::trace!(%proto_id, bytes = bytes.len(), "received");
            muxer
                .received(timestamp, proto_id, bytes.into(), eff)
                .await
                .with_context(|| format!("reading message for protocol {}", proto_id))?;
            eff.send(reader, Read).await;
            Ok(())
        }
        MuxMessage::WantNext(proto_id) => muxer
            .want_next(proto_id, eff)
            .await
            .with_context(|| format!("reading message for protocol {}", proto_id)),
        MuxMessage::Written => {
            *sending = false;
            if let Some((proto_id, bytes)) = muxer.next_segment(eff).await {
                *sending = true;
                eff.send(writer, Header::encode(proto_id, &bytes).into())
                    .await;
            }
            Ok(())
        }
    }
}

async fn read_segment(
    (conn, muxer): (ConnectionId, StageRef<MuxMessage>),
    _token: Read,
    eff: Effects<Read>,
) -> (ConnectionId, StageRef<MuxMessage>) {
    let mut data = Network::new(&eff)
        .recv(conn, HEADER_LEN)
        .await
        .or_terminate(
            &eff,
            async |err| tracing::error!(%err, "failed to receive segment header from network"),
        )
        .await;
    let header = Header::decode(&mut data);

    if header.length == 0 {
        tracing::warn!("received empty segment");
        return eff.terminate().await;
    }

    let data = Network::new(&eff)
        .recv(conn, header.length.into())
        .await
        .or_terminate(
            &eff,
            async |err| tracing::error!(%err, "failed to receive segment data from network"),
        )
        .await;

    eff.send(
        &muxer,
        MuxMessage::FromNetwork(header.timestamp, header.proto_id, data),
    )
    .await;
    (conn, muxer)
}

struct Header {
    timestamp: Timestamp,
    proto_id: ProtocolId<Erased>,
    length: u16,
}
const HEADER_LEN: usize = 8;

impl Header {
    pub fn encode<R: Role>(proto_id: ProtocolId<R>, bytes: impl AsRef<[u8]>) -> Bytes {
        thread_local! {
            static BUFFER: RefCell<BytesMut> = RefCell::new(BytesMut::with_capacity(HEADER_LEN+MAX_SEGMENT_SIZE));
        }
        let bytes = bytes.as_ref();
        BUFFER.with_borrow_mut(move |buffer| {
            buffer.clear();
            Timestamp::now().encode(buffer);
            proto_id.encode(buffer);
            buffer.put_u16(bytes.len() as u16);
            buffer.extend_from_slice(bytes);
            buffer.copy_to_bytes(buffer.remaining())
        })
    }

    pub fn decode(buffer: &mut Bytes) -> Self {
        Self {
            timestamp: Timestamp::decode(buffer),
            proto_id: ProtocolId::decode(buffer),
            length: buffer.get_u16(),
        }
    }
}

#[expect(clippy::disallowed_types)]
type Protocols = HashMap<ProtocolId<Erased>, PerProto>;

#[derive(Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Muxer {
    protocols: Protocols,
    outgoing: Vec<ProtocolId<Erased>>,
    next_out: usize,
}

impl Muxer {
    pub fn new() -> Self {
        Self::default()
    }

    #[instrument(level = Level::DEBUG, skip(self))]
    pub async fn register<M>(
        &mut self,
        proto_id: ProtocolId<Erased>,
        frame: Frame,
        max_buffer: usize,
        handler: StageRef<Bytes>,
        eff: &Effects<M>,
    ) -> anyhow::Result<()> {
        let pp = self.do_register(proto_id, frame, max_buffer, handler);
        pp.want_next(eff).await?;
        Ok(())
    }

    #[instrument(level = Level::DEBUG, skip(self))]
    pub fn buffer(&mut self, proto_id: ProtocolId<Erased>, limit: usize) -> anyhow::Result<()> {
        let pp = self.do_register(proto_id, Frame::Buffer, limit, StageRef::blackhole());
        if limit == 0 {
            tracing::trace!(buffer = pp.incoming.len(), "switching to ignoring mode");
            pp.incoming.clear();
        } else if pp.incoming.len() > limit {
            tracing::warn!(
                buffer = pp.incoming.len(),
                limit,
                "reducing buffer killed the connection"
            );
            anyhow::bail!(
                "reducing buffer ({}) leads to excess data ({})",
                limit,
                pp.incoming.len()
            );
        }
        Ok(())
    }

    fn do_register(
        &mut self,
        proto_id: ProtocolId<Erased>,
        frame: Frame,
        max_buffer: usize,
        handler: StageRef<Bytes>,
    ) -> &mut PerProto {
        if !self.outgoing.contains(&proto_id) {
            self.outgoing.push(proto_id);
        }
        match self.protocols.entry(proto_id) {
            Entry::Occupied(pp) => {
                let pp = pp.into_mut();
                tracing::trace!(want = pp.wanted, "updating registration");
                pp.frame = frame;
                pp.max_buffer = max_buffer;
                pp.handler = handler;
                pp
            }
            Entry::Vacant(pp) => pp.insert(PerProto::new(handler, frame, max_buffer)),
        }
    }

    #[instrument(level = Level::DEBUG, skip_all, fields(proto_id))]
    pub fn outgoing(&mut self, proto_id: ProtocolId<Erased>, bytes: Bytes, sent: CallRef<Sent>) {
        tracing::trace!(proto = %proto_id, bytes = bytes.len(), "enqueueing send");
        #[allow(clippy::expect_used)]
        self.protocols
            .get_mut(&proto_id)
            .ok_or_else(|| anyhow::anyhow!("protocol {} not registered", proto_id))
            .expect("internal error")
            .enqueue_send(bytes, sent);
    }

    #[instrument(level = Level::DEBUG, skip(self))]
    pub async fn next_segment<M>(
        &mut self,
        eff: &Effects<M>,
    ) -> Option<(ProtocolId<Erased>, Bytes)> {
        tracing::trace!(next = self.next_out, "next segment");
        for idx in (self.next_out..self.outgoing.len()).chain(0..self.next_out) {
            let proto_id = self.outgoing[idx];
            #[allow(clippy::expect_used)]
            let proto = self
                .protocols
                .get_mut(&proto_id)
                .expect("invariant violation");
            let Some(bytes) = proto.next_segment(eff).await else {
                tracing::trace!(proto = %proto_id, idx, "no segment");
                continue;
            };
            self.next_out = (idx + 1) % self.outgoing.len();
            tracing::trace!(size = bytes.len(), proto = %proto_id, next = self.next_out, "sending segment");
            return Some((proto_id, bytes));
        }
        None
    }

    #[instrument(level = Level::DEBUG, skip(self, bytes), fields(bytes = bytes.len()))]
    pub async fn received<M>(
        &mut self,
        timestamp: Timestamp,
        proto_id: ProtocolId<Erased>,
        bytes: Bytes,
        eff: &Effects<M>,
    ) -> anyhow::Result<()> {
        if let Some(proto) = self.protocols.get_mut(&proto_id) {
            proto.received(timestamp, bytes, eff).await
        } else {
            anyhow::bail!("received data for unknown protocol {}", proto_id)
        }
    }

    #[instrument(level = Level::DEBUG, skip(self))]
    pub async fn want_next<M>(
        &mut self,
        proto_id: ProtocolId<Erased>,
        eff: &Effects<M>,
    ) -> anyhow::Result<()> {
        #[allow(clippy::expect_used)]
        self.protocols
            .get_mut(&proto_id)
            .ok_or_else(|| anyhow::anyhow!("protocol {} not registered", proto_id))
            .expect("internal error")
            .want_next(eff)
            .await?;
        Ok(())
    }
}

#[derive(PartialEq, serde::Serialize, serde::Deserialize)]
struct PerProto {
    incoming: BytesMut,
    outgoing: BytesMut,
    sent_bytes: usize,
    notifiers: VecDeque<(CallRef<Sent>, usize)>,
    handler: StageRef<Bytes>,
    wanted: usize,
    frame: Frame,
    max_buffer: usize,
}

impl std::fmt::Debug for PerProto {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PerProto")
            .field("incoming", &self.incoming.len())
            .field("outgoing", &self.outgoing.len())
            .field("sent_bytes", &self.sent_bytes)
            .field("notifiers", &self.notifiers)
            .field("handler", &self.handler)
            .field("wanted", &self.wanted)
            .field("frame", &self.frame)
            .field("max_buffer", &self.max_buffer)
            .finish()
    }
}

impl PerProto {
    pub fn new(handler: StageRef<Bytes>, frame: Frame, max_buffer: usize) -> Self {
        Self {
            incoming: BytesMut::with_capacity(max_buffer),
            outgoing: BytesMut::with_capacity(max_buffer),
            sent_bytes: 0,
            notifiers: VecDeque::new(),
            handler,
            wanted: 0,
            frame,
            max_buffer,
        }
    }

    pub async fn received<M>(
        &mut self,
        _timestamp: Timestamp,
        bytes: Bytes,
        eff: &Effects<M>,
    ) -> anyhow::Result<()> {
        if self.max_buffer == 0 {
            tracing::debug!(size = bytes.len(), "ignoring bytes");
            return Ok(());
        }
        tracing::trace!(wanted = self.wanted, "received bytes");
        if self.incoming.len() + bytes.len() > self.max_buffer {
            tracing::info!(
                buffered = self.incoming.len(),
                max_buffer = self.max_buffer,
                "message exceeds buffer"
            );
            anyhow::bail!(
                "message (size {}) plus buffer (size {}) exceeds limit ({})",
                bytes.len(),
                self.incoming.len(),
                self.max_buffer
            );
        }
        self.incoming.extend(&bytes);
        while self.wanted > 0
            && let Some(bytes) = self.frame.try_consume(&mut self.incoming)?
        {
            tracing::trace!(len = bytes.len(), "extracted message");
            eff.send(&self.handler, bytes).await;
            self.wanted -= 1;
        }
        Ok(())
    }

    pub async fn want_next<M>(&mut self, eff: &Effects<M>) -> anyhow::Result<()> {
        tracing::trace!(wanted = self.wanted, "wanting next");
        if !self.incoming.is_empty()
            && let Some(bytes) = self.frame.try_consume(&mut self.incoming)?
        {
            tracing::trace!(len = bytes.len(), "extracted message");
            eff.send(&self.handler, bytes).await;
        } else {
            tracing::trace!("next delivery deferred");
            self.wanted += 1;
        }
        Ok(())
    }

    pub fn enqueue_send(&mut self, bytes: Bytes, sent: CallRef<Sent>) {
        self.outgoing.extend(&bytes);
        self.notifiers
            .push_back((sent, self.sent_bytes + self.outgoing.len()));
    }

    pub async fn next_segment<M>(&mut self, eff: &Effects<M>) -> Option<Bytes> {
        if self.outgoing.is_empty() {
            return None;
        }
        let size = self.outgoing.len().min(MAX_SEGMENT_SIZE);
        self.sent_bytes += size;
        while let Some((_sent, size)) = self.notifiers.front() {
            if self.sent_bytes >= *size {
                #[expect(clippy::expect_used)]
                let (sent, _) = self.notifiers.pop_front().expect("checked above");
                eff.respond(sent, Sent).await;
            } else {
                break;
            }
        }
        Some(self.outgoing.copy_to_bytes(size))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        effects::{RecvEffect, SendEffect},
        protocol::{Initiator, PROTO_HANDSHAKE, PROTO_N2C_CHAIN_SYNC, PROTO_N2N_BLOCK_FETCH},
        socket::ConnectionResource,
    };
    use futures_util::StreamExt;
    use pure_stage::{
        Effect, Instant, StageGraph,
        simulation::{Blocked, SimulationBuilder, SimulationRunning},
        trace_buffer::TraceBuffer,
    };
    use std::{fmt, time::Duration};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        time::timeout,
    };

    /// Tests with real async behaviour unfortunately need real wall clock sleep time to allow
    /// things to propagate or assert that something doesn’t get propagated. If tests below are
    /// flaky then this value may be too small for the machine running the test.
    const SAFE_SLEEP: Duration = Duration::from_millis(400);
    const TIMEOUT: Duration = Duration::from_secs(1);

    async fn s<F: Future>(f: F)
    where
        F::Output: fmt::Debug,
    {
        timeout(SAFE_SLEEP, f).await.unwrap_err();
    }

    async fn t<F: Future>(f: F) -> F::Output {
        timeout(TIMEOUT, f).await.unwrap()
    }

    #[tokio::test]
    async fn test_tcp() {
        let _guard = pure_stage::register_data_deserializer::<MuxMessage>();
        let _guard = pure_stage::register_data_deserializer::<DebugBytes>();
        let _guard = pure_stage::register_effect_deserializer::<SendEffect>();
        let _guard = pure_stage::register_effect_deserializer::<RecvEffect>();
        let _guard = pure_stage::register_data_deserializer::<State>();

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let server_addr = listener.local_addr().unwrap();
        let server_task = tokio::spawn(async move { listener.accept().await.unwrap().0 });

        let network = ConnectionResource::new(65536);
        let conn_id = t(network.connect(vec![server_addr])).await.unwrap();
        let mut tcp = t(server_task).await.unwrap();

        let trace_buffer = TraceBuffer::new_shared(1000, 1000000);
        let trace_guard = TraceBuffer::drop_guard(&trace_buffer);
        let mut graph = SimulationBuilder::default().with_trace_buffer(trace_buffer);

        let mux = graph.stage("mux", super::stage);
        let mux = graph.wire_up(
            mux,
            // FIXME make test-only protocol ID
            State::new(conn_id, &[(PROTO_N2C_CHAIN_SYNC.erase(), 0)]),
        );

        let (output, mut rx) = graph.output::<Bytes>("output", 10);
        let input = graph.input(&mux);

        graph.resources().put(network);

        let mut running = graph.run();
        let join_handle = tokio::spawn(async move {
            loop {
                let blocked = running.run_until_blocked();
                eprintln!("{blocked:?}");
                match blocked {
                    Blocked::Idle => running.await_external_input().await,
                    Blocked::Sleeping { .. } => unreachable!(),
                    Blocked::Deadlock(send_blocks) => panic!("deadlock: {:?}", send_blocks),
                    Blocked::Breakpoint(..) => unreachable!(),
                    Blocked::Busy {
                        external_effects, ..
                    } => {
                        assert!(external_effects > 0);
                        running.await_external_effect().await;
                    }
                    Blocked::Terminated(name) => return name,
                };
            }
        });

        let (cr, cr_rx) = CallRef::channel();
        input
            .send(MuxMessage::Send(
                PROTO_N2C_CHAIN_SYNC.erase(),
                Bytes::copy_from_slice(&[1, 24, 33]).into(),
                cr,
            ))
            .await
            .unwrap();
        let mut buf = [0u8; 11];
        assert_eq!(t(tcp.read_exact(&mut buf)).await.unwrap(), 11);
        t(cr_rx).await.unwrap().cast::<Sent>().unwrap();
        // first four bytes are timestamp; proto ID is 5, length is 3
        assert_eq!(&buf[4..], [0, 5, 0, 3, 1, 24, 33]);

        input
            .send(MuxMessage::Register {
                protocol: PROTO_N2C_CHAIN_SYNC.erase(),
                frame: Frame::OneCborItem,
                handler: output,
                max_buffer: 100,
            })
            .await
            .unwrap();

        t(tcp.write_all(&buf)).await.unwrap();
        t(tcp.flush()).await.unwrap();
        assert_eq!(t(rx.next()).await.unwrap(), Bytes::copy_from_slice(&[1]));
        s(rx.next()).await;
        input
            .send(MuxMessage::WantNext(PROTO_N2C_CHAIN_SYNC.erase()))
            .await
            .unwrap();
        assert_eq!(
            t(rx.next()).await.unwrap(),
            Bytes::copy_from_slice(&[24, 33])
        );

        // wrong protocol ID
        buf[5] += 1;
        t(tcp.write_all(&buf)).await.unwrap();
        t(tcp.flush()).await.unwrap();
        assert_eq!(&t(join_handle).await.unwrap(), mux.name());

        trace_guard.defuse();
    }

    #[test]
    fn test_muxing() {
        let _guard = pure_stage::register_data_deserializer::<MuxMessage>();
        let _guard = pure_stage::register_data_deserializer::<DebugBytes>();
        let _guard = pure_stage::register_effect_deserializer::<SendEffect>();
        let _guard = pure_stage::register_effect_deserializer::<RecvEffect>();
        let _guard = pure_stage::register_data_deserializer::<State>();

        let trace_buffer = TraceBuffer::new_shared(100, 1_000_000);
        let drop_guard = TraceBuffer::drop_guard(&trace_buffer);
        let mut network = SimulationBuilder::default().with_trace_buffer(trace_buffer);
        let mux = network.stage("mux", super::stage);
        let conn_id = ConnectionId::new();
        let mux = network.wire_up(
            mux,
            State::new(
                conn_id,
                // sequence of registration is the sequence of round-robin
                &[
                    (PROTO_N2C_CHAIN_SYNC.erase(), 1024),
                    (PROTO_N2N_BLOCK_FETCH.erase(), 0),
                    (PROTO_HANDSHAKE.erase(), 1),
                ],
            ),
        );

        let mut running = network.run();
        let running = &mut running;

        // set breakpoints to capture interactions with outside world
        running.breakpoint(
            "send",
            |eff| matches!(eff, Effect::External { effect, .. } if effect.is::<SendEffect>()),
        );
        running.breakpoint(
            "recv",
            |eff| matches!(eff, Effect::External { effect, .. } if effect.is::<RecvEffect>()),
        );
        running.breakpoint("spawn", |eff| matches!(eff, Effect::WireStage { .. }));

        // send a message to trigger creation of the writer and reader stages
        let chain_sync = StageRef::named_for_tests("chain_sync");
        running.enqueue_msg(
            &mux,
            [MuxMessage::Register {
                protocol: PROTO_N2C_CHAIN_SYNC.erase(),
                frame: Frame::OneCborItem,
                handler: chain_sync.clone(),
                max_buffer: 1024,
            }],
        );
        let spawn1 = running.run_until_blocked().assert_breakpoint("spawn");
        let writer = spawn1
            .extract_wire_stage(&mux, (conn_id, (*mux).clone()))
            .clone();
        running.handle_effect(spawn1);

        let spawn2 = running.run_until_blocked().assert_breakpoint("spawn");
        let reader = spawn2
            .extract_wire_stage(&mux, (conn_id, (*mux).clone()))
            .clone();
        running.handle_effect(spawn2);

        {
            let mux_name = mux.name().clone();
            let writer = writer.clone();
            let reader = reader.clone();
            running.breakpoint("mux", move |eff| {
                matches!(eff, Effect::Send { from, to, .. } | Effect::Respond { at_stage: from, target: to, .. }
                if from == &mux_name && to != &writer && to != &reader)
            });
        }

        running
            .run_until_blocked()
            .assert_breakpoint("recv")
            .assert_external(
                &reader,
                &RecvEffect {
                    conn: conn_id,
                    bytes: 8,
                },
            );
        running.run_until_blocked().assert_busy([&reader]);

        // send a message towards the network
        let send_msg = |running: &mut SimulationRunning,
                        id: u64,
                        msg: u8,
                        len: usize,
                        proto_id: ProtocolId<Initiator>| {
            let bytes = vec![msg; len];
            let cr = CallRef::fake("fake", id, Instant::at_offset(Duration::ZERO));
            running.enqueue_msg(
                &mux,
                [MuxMessage::Send(
                    proto_id.erase(),
                    Bytes::copy_from_slice(&bytes).into(),
                    cr.dummy(),
                )],
            );
            cr
        };

        let assert_send = |running: &mut SimulationRunning,
                           data: &[(usize, u8)],
                           proto_id: ProtocolId<Initiator>| {
            running
                .run_until_blocked()
                .assert_breakpoint("send")
                .extract_external::<SendEffect>(&writer)
                .assert_frame(conn_id, proto_id.erase(), data);
        };
        let resume_send = |running: &mut SimulationRunning| {
            running
                .resume_external::<SendEffect>(&writer, Ok(()))
                .unwrap();
        };
        let assert_and_resume_send =
            |running: &mut SimulationRunning,
             data: &[(usize, u8)],
             proto_id: ProtocolId<Initiator>| {
                assert_send(running, data, proto_id);
                resume_send(running);
            };
        let assert_respond = |running: &mut SimulationRunning, cr: &CallRef<Sent>| {
            let mux_sent = running.run_until_blocked().assert_breakpoint("mux");
            mux_sent.assert_respond(&mux, cr, Sent);
            running.handle_effect(mux_sent);
        };

        // start write but don't let the writer finish yet
        let cr1 = send_msg(running, 101, 1, 1024, PROTO_N2C_CHAIN_SYNC);
        assert_respond(running, &cr1);
        assert_send(running, &[(1024, 1)], PROTO_N2C_CHAIN_SYNC);

        // put 1024 bytes into the proto buffer
        let cr2 = send_msg(running, 102, 2, 1024, PROTO_N2C_CHAIN_SYNC);
        // put 10 bytes into the proto buffer
        let cr3 = send_msg(running, 103, 3, 10, PROTO_N2C_CHAIN_SYNC);
        // the above are for checking correct responses via the CallRefs

        // fill segments for other two protocols
        let cr4 = send_msg(running, 104, 4, 66000, PROTO_HANDSHAKE);
        let cr5 = send_msg(running, 105, 5, 66000, PROTO_N2N_BLOCK_FETCH);

        resume_send(running);
        assert_and_resume_send(running, &[(65535, 5)], PROTO_N2N_BLOCK_FETCH);
        assert_and_resume_send(running, &[(65535, 4)], PROTO_HANDSHAKE);
        assert_respond(running, &cr2);
        assert_respond(running, &cr3);
        assert_and_resume_send(running, &[(1024, 2), (10, 3)], PROTO_N2C_CHAIN_SYNC);
        assert_respond(running, &cr5);
        assert_and_resume_send(running, &[(465, 5)], PROTO_N2N_BLOCK_FETCH);
        assert_respond(running, &cr4);
        assert_and_resume_send(running, &[(465, 4)], PROTO_HANDSHAKE);

        let recv_header = RecvEffect {
            conn: conn_id,
            bytes: 8,
        };
        let recv_msg = |running: &mut SimulationRunning,
                        proto_id: ProtocolId<Initiator>,
                        bytes: &[u8],
                        recv: &[&[u8]]| {
            let mut msg = Header::encode(proto_id, bytes);
            running
                .resume_external::<RecvEffect>(&reader, Ok(msg.split_to(HEADER_LEN).into()))
                .unwrap();
            running
                .run_until_blocked()
                .assert_breakpoint("recv")
                .assert_external(
                    &reader,
                    &RecvEffect {
                        conn: conn_id,
                        bytes: msg.len(),
                    },
                );
            running
                .resume_external::<RecvEffect>(&reader, Ok(msg.into()))
                .unwrap();
            for recv in recv {
                if recv.is_empty() {
                    running
                        .run_until_blocked()
                        .assert_breakpoint("recv")
                        .assert_external(&reader, &recv_header);
                    continue;
                }
                running
                    .run_until_blocked()
                    .assert_breakpoint("mux")
                    .assert_send(&mux, &chain_sync, Bytes::copy_from_slice(recv));
                running.resume_send(&mux, &chain_sync, None).unwrap();
                running.enqueue_msg(&mux, [MuxMessage::WantNext(proto_id.erase())]);
            }
            // running.run_until_blocked().assert_busy([&reader]);
        };

        // send CBOR 1 followed by incomplete CBOR; "recv" effect always happens second
        recv_msg(running, PROTO_N2C_CHAIN_SYNC, &[1, 24], &[&[1], &[]]);
        // send CBOR 25 continuation followed by CBOR 3
        recv_msg(
            running,
            PROTO_N2C_CHAIN_SYNC,
            &[25, 3],
            &[&[24, 25], &[], &[3]],
        );

        // test buffer size violation
        recv_msg(running, PROTO_HANDSHAKE, &[1, 2, 3], &[]);
        running.run_until_blocked().assert_terminated(mux.name());

        drop_guard.defuse();
    }

    trait AssertBytes {
        fn assert_frame(
            &self,
            conn: ConnectionId,
            proto_id: ProtocolId<Erased>,
            data: &[(usize, u8)],
        );
    }
    impl AssertBytes for SendEffect {
        fn assert_frame(
            &self,
            conn: ConnectionId,
            proto_id: ProtocolId<Erased>,
            data: &[(usize, u8)],
        ) {
            assert_eq!(self.conn, conn);
            let mut header = self.data.slice(..HEADER_LEN);
            let header = Header::decode(&mut header);
            assert_eq!(header.proto_id, proto_id);
            assert_eq!(
                header.length as usize,
                data.iter().map(|(len, _)| len).sum::<usize>()
            );
            let mut bytes = self.data.slice(HEADER_LEN..);
            for &(len, msg) in data {
                assert_eq!(&bytes.split_to(len), &vec![msg; len]);
            }
        }
    }
}
