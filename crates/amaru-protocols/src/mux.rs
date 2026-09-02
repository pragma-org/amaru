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

#[expect(clippy::disallowed_types)]
use std::collections::HashMap;
use std::{
    cell::RefCell,
    collections::{VecDeque, hash_map::Entry},
    num::{NonZeroU16, NonZeroUsize},
    time::{Duration, SystemTime},
};

use amaru_kernel::{NonEmptyBytes, Peer};
use amaru_observability::{Instrument, debug, debug_span, error, info, trace, warn};
use amaru_ouroboros::ConnectionId;
use amaru_pure_stage::{Effects, Instant, OrTerminateWith, StageRef, TryInStage, Void};
use anyhow::Context;
use bytes::{Buf, BufMut, Bytes, BytesMut, TryGetError};
use cbor_data::{Cbor, ErrorKind, ParseError};

use crate::{
    network_effects::{Network, NetworkOps},
    protocol::{Erased, ProtocolId, Role, RoleT},
};

pub fn register_deserializers() -> amaru_pure_stage::DeserializerGuards {
    vec![
        amaru_pure_stage::register_data_deserializer::<MuxMessage>().boxed(),
        amaru_pure_stage::register_data_deserializer::<NonEmptyBytes>().boxed(),
        amaru_pure_stage::register_data_deserializer::<State>().boxed(),
        amaru_pure_stage::register_data_deserializer::<HandlerMessage>().boxed(),
        amaru_pure_stage::register_data_deserializer::<Sent>().boxed(),
        amaru_pure_stage::register_data_deserializer::<Read>().boxed(),
        amaru_pure_stage::register_data_deserializer::<OutgoingSdu>().boxed(),
        amaru_pure_stage::register_data_deserializer::<Peer>().boxed(),
        amaru_pure_stage::register_data_deserializer::<(ConnectionId, StageRef<MuxMessage>, Role, Peer)>().boxed(),
    ]
}

const MAX_SEGMENT_SIZE: usize = 65535;

/// Mux SDU assembly/send timer during the first Handshake on a bearer.
pub const SDU_TIMEOUT_HANDSHAKE: Duration = Duration::from_secs(10);
/// Mux SDU assembly/send timer after that Handshake has finished.
pub const SDU_TIMEOUT_ESTABLISHED: Duration = Duration::from_secs(30);

const HEADER_LEADING_EDGE: NonZeroUsize = NonZeroUsize::MIN;
const HEADER_REST: NonZeroUsize = NonZeroUsize::new(7).expect("7 is non-zero");

/// microseconds part of the wall clock time
#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Timestamp(u32);

impl Timestamp {
    pub fn now() -> Self {
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

    pub fn from_instant(instant: Instant) -> Self {
        Self(instant.sim_elapsed().as_micros() as u32)
    }

    fn decode(buffer: &mut Bytes) -> Result<Self, TryGetError> {
        Ok(Self(buffer.try_get_u32()?))
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
    pub fn try_consume(&self, data: &mut BytesMut) -> Result<Option<NonEmptyBytes>, ParseError> {
        match self {
            Frame::OneCborItem => match Cbor::checked_prefix(data) {
                Ok((item, _rest)) => {
                    let item = data.copy_to_bytes(item.as_slice().len());
                    #[expect(clippy::expect_used)]
                    Ok(Some(item.try_into().expect("guaranteed by CBOR standard")))
                }
                Err(e) if matches!(e.kind(), ErrorKind::UnexpectedEof(_)) => Ok(None),
                Err(e) => Err(e),
            },
            Frame::Buffer => Ok(None),
        }
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum HandlerMessage {
    Registered(ProtocolId<Erased>),
    FromNetwork(NonEmptyBytes),
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Sent;

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Read {
    pub sdu_timeout: Duration,
}

/// One mux SDU to write, with the assembly/send timer that applies to that write.
#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct OutgoingSdu {
    pub data: NonEmptyBytes,
    pub timeout: Duration,
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum MuxMessage {
    /// Register the given protocol with its ID so that data will be fed into it
    ///
    /// Note that the handler explicitly needs to request each network message by sending `WantNext`.
    /// This is necessary to allow proper handling of TCP simultaneous open in the handshake protocol.
    Register { protocol: ProtocolId<Erased>, frame: Frame, handler: StageRef<HandlerMessage>, max_buffer: usize },
    /// Buffer incoming data for this protocol ID up to the given limit
    /// (this should be followed by Register eventually, to then consume the data)
    ///
    /// Setting the size to zero means that data are dropped without begin buffered
    /// and without tearing down the connection.
    Buffer(ProtocolId<Erased>, usize),
    /// Send the given message on the protocol ID and notify when enqueued in TCP buffer
    Send(ProtocolId<Erased>, NonEmptyBytes, StageRef<Sent>),
    /// internal message coming from the TCP stream reader
    FromNetwork(Timestamp, ProtocolId<Erased>, NonEmptyBytes),
    /// Notify that the segment has been written to the TCP stream
    Written,
    /// Permit the next invocation of the Protocol with data from the network.
    WantNext(ProtocolId<Erased>),
    /// Reading or writing error occurred
    Terminate,
    /// Switch the SDU assembly/send timer (10s during first Handshake, 30s afterwards).
    SetSduTimeout(Duration),
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct State {
    conn: Connection,
    muxer: Muxer,
    sending: bool,
    peer: Peer,
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
enum Connection {
    Unint(ConnectionId),
    Init(StageRef<OutgoingSdu>, StageRef<Read>),
}

impl State {
    /// Create a new state with the given connection ID and buffering the given protocols.
    ///
    /// Note that upon receiving the first message, the stage will start reading from the network.
    /// Any data received for unregistered protocols will lead to stage termination.
    pub fn new(conn: ConnectionId, buffer: &[(ProtocolId<Erased>, usize)], role: Role, peer: Peer) -> Self {
        let mut muxer = Muxer::new(role);
        for &(proto_id, limit) in buffer {
            #[expect(clippy::expect_used)]
            muxer.buffer(proto_id, limit).expect("no buffered data yet");
        }
        Self { conn: Connection::Unint(conn), muxer, sending: false, peer }
    }

    pub async fn init(
        &mut self,
        eff: &mut Effects<MuxMessage>,
    ) -> (&mut Muxer, &mut bool, &StageRef<OutgoingSdu>, &StageRef<Read>) {
        match &mut self.conn {
            Connection::Unint(conn) => {
                let writer = eff
                    .stage(
                        format!("writer-{}", conn),
                        move |(conn, muxer, role, peer): (ConnectionId, StageRef<MuxMessage>, Role, Peer),
                              OutgoingSdu { data, timeout }: OutgoingSdu,
                              eff| async move {
                            Network::new(&eff)
                                .send(conn, data, Some(timeout))
                                .or_terminate_with(&eff, async |err| {
                                    error!(
                                        protocols::mux::FAILED,
                                        role = role.to_string(),
                                        operation = "send",
                                        peer,
                                        error = err.to_string()
                                    );
                                })
                                .await;
                            eff.send(&muxer, MuxMessage::Written).await;
                            (conn, muxer, role, peer)
                        },
                    )
                    .await;
                let writer = eff.supervise(writer, MuxMessage::Terminate);
                let writer = eff.wire_up(writer, (*conn, eff.me(), self.muxer.role(), self.peer)).await;
                let reader = eff.stage(format!("reader-{}", conn), read_segment).await;
                let reader = eff.supervise(reader, MuxMessage::Terminate);
                let reader = eff.wire_up(reader, (*conn, eff.me(), self.muxer.role(), self.peer)).await;
                eff.send(&reader, Read { sdu_timeout: self.muxer.sdu_timeout }).await;
                self.conn = Connection::Init(writer, reader);
            }
            Connection::Init(..) => {}
        }
        let Connection::Init(writer, reader) = &self.conn else { unreachable!() };
        (&mut self.muxer, &mut self.sending, writer, reader)
    }
}

pub async fn stage(mut state: State, msg: MuxMessage, mut eff: Effects<MuxMessage>) -> State {
    let peer = state.peer;
    let (muxer, sending, writer, reader) = state.init(&mut eff).await;

    handle_msg(msg, &eff, muxer, sending, writer, reader)
        .await
        .or_terminate(&eff, async |error| {
            use std::fmt::Write;
            let mut err = String::new();
            for error in error.chain() {
                if !err.is_empty() {
                    err.push_str(" <- ");
                }
                write!(&mut err, "{}", error).ok();
            }
            error!(
                protocols::mux::FAILED,
                peer,
                role = muxer.role().to_string().to_string().to_string().to_string(),
                operation = "muxing",
                error = err.to_string()
            );
        })
        .await;

    state
}

async fn handle_msg(
    msg: MuxMessage,
    eff: &Effects<MuxMessage>,
    muxer: &mut Muxer,
    sending: &mut bool,
    writer: &StageRef<OutgoingSdu>,
    reader: &StageRef<Read>,
) -> anyhow::Result<()> {
    match msg {
        MuxMessage::Register { protocol, frame, handler, max_buffer } => {
            muxer.register(protocol, frame, max_buffer, handler, eff).await
        }
        MuxMessage::Buffer(proto_id, limit) => muxer.buffer(proto_id, limit),
        MuxMessage::Send(proto_id, bytes, sent) => {
            trace!(protocols::mux::protocol::SEND, proto_id = proto_id.to_string(), bytes = bytes.len().get() as u64);
            muxer.outgoing(proto_id, bytes.into(), sent);
            if !*sending && let Some((proto_id, bytes)) = muxer.next_segment(eff).await {
                *sending = true;
                let header = muxer.encode_header(eff, proto_id, &bytes).await;
                eff.send(writer, OutgoingSdu { data: header, timeout: muxer.sdu_timeout }).await;
            }
            Ok(())
        }
        MuxMessage::FromNetwork(timestamp, proto_id, bytes) => {
            trace!(
                protocols::mux::protocol::RECEIVED,
                proto_id = proto_id.to_string(),
                bytes = bytes.len().get() as u64
            );
            muxer
                .received(timestamp, proto_id.opposite(), bytes.into(), eff)
                .await
                .with_context(|| format!("reading network message for protocol {}", proto_id))?;
            eff.send(reader, Read { sdu_timeout: muxer.sdu_timeout }).await;
            Ok(())
        }
        MuxMessage::WantNext(proto_id) => {
            muxer.want_next(proto_id, eff).await.with_context(|| format!("reading message for protocol {}", proto_id))
        }
        MuxMessage::Written => {
            *sending = false;
            if let Some((proto_id, bytes)) = muxer.next_segment(eff).await {
                *sending = true;
                let header = muxer.encode_header(eff, proto_id, &bytes).await;
                eff.send(writer, OutgoingSdu { data: header, timeout: muxer.sdu_timeout }).await;
            }
            Ok(())
        }
        MuxMessage::Terminate => {
            debug!(protocols::mux::TERMINATING, role = muxer.role().to_string().to_string().to_string().to_string());
            eff.terminate::<Void>().await;
            Ok(())
        }
        MuxMessage::SetSduTimeout(timeout) => {
            muxer.sdu_timeout = timeout;
            Ok(())
        }
    }
}

async fn read_segment(
    (conn, muxer, role, peer): (ConnectionId, StageRef<MuxMessage>, Role, Peer),
    Read { sdu_timeout }: Read,
    eff: Effects<Read>,
) -> (ConnectionId, StageRef<MuxMessage>, Role, Peer) {
    let header = loop {
        let first = Network::new(&eff)
            .recv(conn, HEADER_LEADING_EDGE, None)
            .or_terminate_with(&eff, async |err| {
                error!(
                    protocols::mux::FAILED,
                    role = role.to_string(),
                    peer,
                    operation = "recv_header_leading_edge",
                    error = err.to_string()
                );
            })
            .await;

        let started = eff.clock().await;
        let rest = Network::new(&eff)
            .recv(conn, HEADER_REST, Some(sdu_timeout))
            .or_terminate_with(&eff, async |err| {
                error!(
                    protocols::mux::FAILED,
                    role = role.to_string(),
                    peer,
                    operation = "recv_header_rest",
                    error = err.to_string()
                );
            })
            .await;

        let mut header_bytes = BytesMut::with_capacity(HEADER_LEN.get());
        header_bytes.extend_from_slice(first.as_ref());
        header_bytes.extend_from_slice(rest.as_ref());
        let mut header_bytes = header_bytes.freeze();

        let Some(header) = Header::decode(&mut header_bytes)
            .or_terminate(&eff, async |err| {
                error!(
                    protocols::mux::FAILED,
                    peer,
                    role = role.to_string(),
                    operation = "decode_header",
                    error = err.to_string()
                );
            })
            .await
        else {
            info!(protocols::mux::EMPTY_SEGMENT, peer, role = role.to_string());
            continue;
        };

        let now = eff.clock().await;
        let remaining = sdu_timeout.saturating_sub(now.saturating_since(started));
        if remaining.is_zero() {
            error!(
                protocols::mux::FAILED,
                peer,
                role = role.to_string(),
                operation = "recv_data",
                error = "sdu timeout (no time left for payload)"
            );
            return eff.terminate().await;
        }

        let data = Network::new(&eff)
            .recv(conn, header.length.into(), Some(remaining))
            .or_terminate_with(&eff, async |err| {
                error!(
                    protocols::mux::FAILED,
                    peer,
                    role = role.to_string(),
                    operation = "recv_data",
                    error = err.to_string()
                );
            })
            .await;

        break (header, data);
    };

    let (header, data) = header;
    eff.send(&muxer, MuxMessage::FromNetwork(header.timestamp, header.proto_id, data)).await;
    (conn, muxer, role, peer)
}

/// A header for a segment of data.
///
/// While the network spec doesn't explicitly forbid sending frames without payload data,
/// we never do that and our code will just ignore such frames.
struct Header {
    timestamp: Timestamp,
    proto_id: ProtocolId<Erased>,
    length: NonZeroU16,
}
const HEADER_LEN: NonZeroUsize = NonZeroUsize::new(8).expect("8 is a valid non-zero size");

impl Header {
    pub fn encode<R: RoleT>(proto_id: ProtocolId<R>, bytes: impl AsRef<[u8]>, timestamp: Timestamp) -> NonEmptyBytes {
        thread_local! {
            static BUFFER: RefCell<BytesMut> = RefCell::new(BytesMut::with_capacity(HEADER_LEN.get() + MAX_SEGMENT_SIZE));
        }
        let bytes = bytes.as_ref();
        BUFFER.with_borrow_mut(move |buffer| {
            buffer.clear();
            timestamp.encode(buffer);
            proto_id.encode(buffer);
            buffer.put_u16(bytes.len() as u16);
            buffer.extend_from_slice(bytes);
            #[expect(clippy::expect_used)]
            buffer.copy_to_bytes(buffer.remaining()).try_into().expect("guaranteed by writing to the buffer")
        })
    }

    pub fn decode(buffer: &mut Bytes) -> Result<Option<Self>, TryGetError> {
        let timestamp = Timestamp::decode(buffer)?;
        let proto_id = ProtocolId::decode(buffer)?;
        let length = buffer.try_get_u16()?;
        Ok(NonZeroU16::new(length).map(|length| Self { timestamp, proto_id, length }))
    }
}

#[expect(clippy::disallowed_types)]
type Protocols = HashMap<ProtocolId<Erased>, PerProto>;

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Muxer {
    protocols: Protocols,
    outgoing: Vec<ProtocolId<Erased>>,
    next_out: usize,
    role: Role,
    sdu_timeout: Duration,
}

impl Muxer {
    pub fn new(role: Role) -> Self {
        Self {
            protocols: Protocols::new(),
            outgoing: Vec::new(),
            next_out: 0,
            role,
            sdu_timeout: SDU_TIMEOUT_HANDSHAKE,
        }
    }

    pub fn role(&self) -> Role {
        self.role
    }

    async fn encode_header<M>(
        &mut self,
        eff: &Effects<M>,
        proto_id: ProtocolId<Erased>,
        bytes: &Bytes,
    ) -> NonEmptyBytes {
        let instant = eff.clock().await;
        let timestamp = Timestamp::from_instant(instant);
        Header::encode(proto_id, bytes, timestamp)
    }

    pub async fn register<M>(
        &mut self,
        proto_id: ProtocolId<Erased>,
        frame: Frame,
        max_buffer: usize,
        handler: StageRef<HandlerMessage>,
        eff: &Effects<M>,
    ) -> anyhow::Result<()> {
        async {
            eff.send(&handler, HandlerMessage::Registered(proto_id)).await;
            self.do_register(proto_id, frame, max_buffer, handler);
            Ok(())
        }
        .instrument(debug_span!(protocols::mux::protocol::REGISTER,))
        .await
    }

    pub fn buffer(&mut self, proto_id: ProtocolId<Erased>, limit: usize) -> anyhow::Result<()> {
        let _span = debug_span!(protocols::mux::protocol::BUFFER,);
        let _guard = _span.enter();

        let pp = self.do_register(proto_id, Frame::Buffer, limit, StageRef::blackhole());
        if limit == 0 {
            trace!(protocols::mux::protocol::BUFFER_IGNORING, buffer = pp.incoming.len());
            pp.incoming.clear();
        } else if pp.incoming.len() > limit {
            warn!(protocols::mux::protocol::BUFFER_OVERFLOW, buffer = pp.incoming.len(), limit);
            anyhow::bail!("reducing buffer ({}) leads to excess data ({})", limit, pp.incoming.len());
        }
        Ok(())
    }

    fn do_register(
        &mut self,
        proto_id: ProtocolId<Erased>,
        frame: Frame,
        max_buffer: usize,
        handler: StageRef<HandlerMessage>,
    ) -> &mut PerProto {
        if !self.outgoing.contains(&proto_id) {
            self.outgoing.push(proto_id);
        }
        match self.protocols.entry(proto_id) {
            Entry::Occupied(pp) => {
                let pp = pp.into_mut();
                trace!(protocols::mux::protocol::WANT_UPDATED, want = pp.wanted);
                pp.frame = frame;
                pp.max_buffer = max_buffer;
                pp.handler = handler;
                pp
            }
            Entry::Vacant(pp) => pp.insert(PerProto::new(handler, frame, max_buffer)),
        }
    }

    pub fn outgoing(&mut self, proto_id: ProtocolId<Erased>, bytes: Bytes, sent: StageRef<Sent>) {
        let _span = debug_span!(
            protocols::mux::protocol::OUTGOING,
            proto_id = format!("{}", proto_id),
            bytes = bytes.len() as u64
        );
        let _guard = _span.enter();

        trace!(protocols::mux::protocol::ENQUEUE, proto_id = proto_id.to_string(), bytes = bytes.len() as u64);
        #[allow(clippy::expect_used)]
        self.protocols
            .get_mut(&proto_id)
            .ok_or_else(|| anyhow::anyhow!("protocol {} not registered", proto_id))
            .expect("internal error")
            .enqueue_send(bytes, sent);
    }

    pub async fn next_segment<M>(&mut self, eff: &Effects<M>) -> Option<(ProtocolId<Erased>, Bytes)> {
        async {
            for idx in (self.next_out..self.outgoing.len()).chain(0..self.next_out) {
                let proto_id = self.outgoing[idx];
                #[allow(clippy::expect_used)]
                let proto = self.protocols.get_mut(&proto_id).expect("invariant violation");
                let Some(bytes) = proto.next_segment(eff).await else {
                    continue;
                };
                self.next_out = (idx + 1) % self.outgoing.len();
                trace!(
                    protocols::mux::protocol::SEGMENT_SENT,
                    proto_id = proto_id.to_string(),
                    bytes = bytes.len() as u64,
                    next = self.next_out as u64
                );
                return Some((proto_id, bytes));
            }
            None
        }
        .instrument(debug_span!(protocols::mux::protocol::NEXT_SEGMENT,))
        .await
    }

    pub async fn received<M>(
        &mut self,
        timestamp: Timestamp,
        proto_id: ProtocolId<Erased>,
        bytes: Bytes,
        eff: &Effects<M>,
    ) -> anyhow::Result<()> {
        let byte_len = bytes.len() as u64;
        async {
            if let Some(proto) = self.protocols.get_mut(&proto_id) {
                proto.received(timestamp, bytes, eff).await
            } else {
                anyhow::bail!("received data for unknown protocol {}", proto_id)
            }
        }
        .instrument(debug_span!(protocols::mux::protocol::RECEIVED, bytes = byte_len))
        .await
    }

    pub async fn want_next<M>(&mut self, proto_id: ProtocolId<Erased>, eff: &Effects<M>) -> anyhow::Result<()> {
        async {
            #[allow(clippy::expect_used)]
            self.protocols
                .get_mut(&proto_id)
                .ok_or_else(|| anyhow::anyhow!("protocol {} not registered", proto_id))
                .expect("internal error")
                .want_next(eff)
                .await?;
            Ok(())
        }
        .instrument(debug_span!(protocols::mux::protocol::WANT_NEXT,))
        .await
    }
}

#[derive(PartialEq, serde::Serialize, serde::Deserialize)]
struct PerProto {
    incoming: BytesMut,
    outgoing: BytesMut,
    sent_bytes: usize,
    notifiers: VecDeque<(StageRef<Sent>, usize)>,
    handler: StageRef<HandlerMessage>,
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
    pub fn new(handler: StageRef<HandlerMessage>, frame: Frame, max_buffer: usize) -> Self {
        Self {
            incoming: BytesMut::new(),
            outgoing: BytesMut::new(),
            sent_bytes: 0,
            notifiers: VecDeque::new(),
            handler,
            wanted: 0,
            frame,
            max_buffer,
        }
    }

    pub async fn received<M>(&mut self, _timestamp: Timestamp, bytes: Bytes, eff: &Effects<M>) -> anyhow::Result<()> {
        if self.max_buffer == 0 {
            debug!(protocols::mux::protocol::IGNORING_BYTES, bytes = bytes.len());
            return Ok(());
        }
        trace!(protocols::mux::protocol::BYTES_RECEIVED, wanted = self.wanted);
        if self.incoming.len() + bytes.len() > self.max_buffer {
            info!(
                protocols::mux::protocol::BUFFER_EXCEEDED,
                buffered = self.incoming.len(),
                max_buffer = self.max_buffer
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
            trace!(protocols::mux::protocol::MESSAGE_EXTRACTED, bytes = bytes.len().get());
            eff.send(&self.handler, HandlerMessage::FromNetwork(bytes)).await;
            self.wanted -= 1;
        }
        Ok(())
    }

    pub async fn want_next<M>(&mut self, eff: &Effects<M>) -> anyhow::Result<()> {
        trace!(protocols::mux::protocol::BYTES_RECEIVED, wanted = self.wanted);
        if !self.incoming.is_empty()
            && let Some(bytes) = self.frame.try_consume(&mut self.incoming)?
        {
            trace!(protocols::mux::protocol::MESSAGE_EXTRACTED, bytes = bytes.len().get());
            eff.send(&self.handler, HandlerMessage::FromNetwork(bytes)).await;
        } else {
            trace!(protocols::mux::protocol::DELIVERY_DEFERRED);
            self.wanted += 1;
        }
        Ok(())
    }

    pub fn enqueue_send(&mut self, bytes: Bytes, sent: StageRef<Sent>) {
        self.outgoing.extend(&bytes);
        self.notifiers.push_back((sent, self.sent_bytes + self.outgoing.len()));
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
                eff.send(&sent, Sent).await;
            } else {
                break;
            }
        }
        Some(self.outgoing.copy_to_bytes(size))
    }
}

#[cfg(test)]
mod tests {
    use std::{fmt, sync::Arc, time::Duration};

    use amaru_network::connection::TokioConnections;
    use amaru_ouroboros::ConnectionsResource;
    use amaru_ouroboros_traits::ConnectionProvider;
    use amaru_pure_stage::{
        Effect, ExternalEffect, Name, StageGraph,
        simulation::{Blocked, Run, SimulationBuilder, SimulationRunning},
        tokio::TokioBuilder,
        trace_buffer::TraceBuffer,
    };
    use futures_util::StreamExt;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        runtime::Handle,
        time::timeout,
    };
    use tracing_subscriber::EnvFilter;

    use super::*;
    use crate::{
        network_effects::{ReceiveError, RecvEffect, SendEffect, SendError},
        protocol::{Initiator, PROTO_HANDSHAKE, PROTO_N2N_BLOCK_FETCH, PROTO_TEST, Responder},
    };

    /// Tests with real async behaviour unfortunately need real wall clock sleep time to allow
    /// things to propagate or assert that something doesn’t get propagated. If tests below are
    /// flaky then this value may be too small for the machine running the test.
    const SAFE_SLEEP: Duration = Duration::from_millis(400);
    const TIMEOUT: Duration = Duration::from_secs(1);

    fn test_peer() -> Peer {
        Peer::for_test(3007)
    }

    #[expect(clippy::wildcard_enum_match_arm)]
    fn external<T: ExternalEffect>(effect: &Effect) -> &T {
        match effect {
            Effect::External { effect, .. } => {
                effect.cast_ref::<T>().unwrap_or_else(|| panic!("expected {}", std::any::type_name::<T>()))
            }
            other => panic!("expected External, got {other:?}"),
        }
    }

    fn wire_child_name(
        running: &SimulationRunning,
        parent: &Name,
        initial: (ConnectionId, StageRef<MuxMessage>, Role, Peer),
    ) -> Name {
        let hit = running.breakpoint_effect();
        #[expect(clippy::wildcard_enum_match_arm)]
        match hit.effect() {
            Effect::WireStage { at_stage, name, initial_state, .. } => {
                assert_eq!(at_stage, parent);
                let got = initial_state
                    .cast_ref::<(ConnectionId, StageRef<MuxMessage>, Role, Peer)>()
                    .expect("wire-up initial state");
                assert_eq!(got, &initial);
                name.clone()
            }
            other => panic!("expected WireStage, got {other:?}"),
        }
    }

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
        let _guard = amaru_pure_stage::register_data_deserializer::<MuxMessage>();
        let _guard = amaru_pure_stage::register_data_deserializer::<NonEmptyBytes>();
        let _guard = amaru_pure_stage::register_effect_deserializer::<SendEffect>();
        let _guard = amaru_pure_stage::register_effect_deserializer::<RecvEffect>();
        let _guard = amaru_pure_stage::register_data_deserializer::<State>();

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let server_addr = listener.local_addr().unwrap();
        let server_task = tokio::spawn(async move { listener.accept().await.unwrap().0 });

        let network = TokioConnections::new(65536);
        let conn_id = t(network.connect(Peer::try_from(server_addr).unwrap(), Duration::from_secs(5))).await.unwrap();
        let mut tcp = t(server_task).await.unwrap();

        let trace_buffer = TraceBuffer::new_shared(1000, 1000000);
        let trace_guard = TraceBuffer::drop_guard(&trace_buffer);
        let mut graph = SimulationBuilder::default().with_trace_buffer(trace_buffer);

        let mux = graph.stage("mux", super::stage);
        let mux = graph.wire_up(mux, State::new(conn_id, &[(PROTO_TEST.erase(), 0)], Role::Initiator, test_peer()));

        let (output, mut rx) = graph.output::<HandlerMessage>("output", 10);
        let (sent, mut sent_rx) = graph.output::<Sent>("sent", 10);
        let input = graph.input(&mux);

        graph.resources().put::<ConnectionsResource>(Arc::new(network));

        let mut running = graph.run(&tokio::runtime::Handle::current());
        let join_handle = tokio::spawn(async move {
            loop {
                let blocked = running.run(Run::skip_wakeups());
                eprintln!("{blocked:?}");
                match blocked {
                    Blocked::Idle => running.await_external_input().await,
                    Blocked::Sleeping { .. } => unreachable!(),
                    Blocked::Deadlock(send_blocks) => panic!("deadlock: {:?}", send_blocks),
                    Blocked::Breakpoint(..) => unreachable!(),
                    Blocked::Busy { external_effects, .. } => {
                        assert!(external_effects > 0);
                        running.await_external_effect().await;
                    }
                    Blocked::Terminated(name) => return name,
                };
            }
        });

        input
            .send(MuxMessage::Send(PROTO_TEST.erase(), Bytes::copy_from_slice(&[1, 24, 33]).try_into().unwrap(), sent))
            .await
            .unwrap();
        let mut buf = [0u8; 11];
        assert_eq!(t(tcp.read_exact(&mut buf)).await.unwrap(), 11);
        t(sent_rx.next()).await.unwrap();
        // first four bytes are timestamp; proto ID is 257 (0x0101), length is 3
        assert_eq!(&buf[4..], [1, 1, 0, 3, 1, 24, 33]);

        input
            .send(MuxMessage::Register {
                protocol: PROTO_TEST.erase(),
                frame: Frame::OneCborItem,
                handler: output,
                max_buffer: 100,
            })
            .await
            .unwrap();
        assert_eq!(t(rx.next()).await.unwrap(), HandlerMessage::Registered(PROTO_TEST.erase()));

        input.send(MuxMessage::WantNext(PROTO_TEST.erase())).await.unwrap();

        // need to flip role bit before sending as responses
        buf[4] |= 0x80;

        t(tcp.write_all(&buf)).await.unwrap();
        t(tcp.flush()).await.unwrap();
        assert_eq!(t(rx.next()).await.unwrap(), HandlerMessage::FromNetwork(NonEmptyBytes::from_slice(&[1]).unwrap()));
        s(rx.next()).await;
        input.send(MuxMessage::WantNext(PROTO_TEST.erase())).await.unwrap();
        assert_eq!(
            t(rx.next()).await.unwrap(),
            HandlerMessage::FromNetwork(NonEmptyBytes::from_slice(&[24, 33]).unwrap())
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
        let _ = tracing_subscriber::fmt().with_env_filter(EnvFilter::from_default_env()).with_test_writer().try_init();

        let _guard = amaru_pure_stage::register_data_deserializer::<MuxMessage>();
        let _guard = amaru_pure_stage::register_data_deserializer::<NonEmptyBytes>();
        let _guard = amaru_pure_stage::register_effect_deserializer::<SendEffect>();
        let _guard = amaru_pure_stage::register_effect_deserializer::<RecvEffect>();
        let _guard = amaru_pure_stage::register_data_deserializer::<State>();
        let _guard = amaru_pure_stage::register_data_deserializer::<Peer>();
        let _guard = amaru_pure_stage::register_data_deserializer::<(ConnectionId, StageRef<MuxMessage>, Role, Peer)>();

        let trace_buffer = TraceBuffer::new_shared(100, 1_000_000);
        let drop_guard = TraceBuffer::drop_guard(&trace_buffer);
        let mut network = SimulationBuilder::default().with_trace_buffer(trace_buffer);
        let mux = network.stage("mux", super::stage);
        let conn_id = ConnectionId::initial();
        let mux = network.wire_up(
            mux,
            State::new(
                conn_id,
                // sequence of registration is the sequence of round-robin
                &[(PROTO_TEST.erase(), 1024), (PROTO_N2N_BLOCK_FETCH.erase(), 0), (PROTO_HANDSHAKE.erase(), 1)],
                Role::Initiator,
                test_peer(),
            ),
        );

        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut running = network.run(rt.handle());
        let running = &mut running;

        // set breakpoints to capture interactions with outside world
        running.breakpoint("send", |eff| matches!(eff, Effect::External { effect, .. } if effect.is::<SendEffect>()));
        running.breakpoint("recv", |eff| matches!(eff, Effect::External { effect, .. } if effect.is::<RecvEffect>()));
        running.breakpoint("spawn", |eff| matches!(eff, Effect::WireStage { .. }));

        // send a message to trigger creation of the writer and reader stages
        let chain_sync = StageRef::named_for_tests("chain_sync");
        running.enqueue_msg(
            &mux,
            [MuxMessage::Register {
                protocol: PROTO_TEST.erase(),
                frame: Frame::OneCborItem,
                handler: chain_sync.clone(),
                max_buffer: 1024,
            }],
        );
        running.run(Run::skip_wakeups()).assert_breakpoint("spawn");
        let writer = wire_child_name(running, mux.name(), (conn_id, (*mux).clone(), Role::Initiator, test_peer()));
        running.run(Run::skip_wakeups()).assert_breakpoint("spawn");
        let reader = wire_child_name(running, mux.name(), (conn_id, (*mux).clone(), Role::Initiator, test_peer()));

        {
            let mux_name = mux.name().clone();
            let writer = writer.clone();
            let reader = reader.clone();
            running.breakpoint(
                "mux",
                move |eff| matches!(eff, Effect::Send { from, to, .. } if from == &mux_name && to != &writer && to != &reader),
            );
        }

        running.run(Run::skip_wakeups()).assert_breakpoint("recv");
        {
            let hit = running.breakpoint_effect();
            let got = external::<RecvEffect>(hit.effect());
            assert_eq!(got, &RecvEffect::leading_edge(conn_id));
        }
        running.discard_breakpoint();
        running.run(Run::skip_wakeups()).assert_breakpoint("mux");
        {
            let hit = running.breakpoint_effect();
            let Effect::Send { to, msg, .. } = hit.effect() else {
                panic!("expected send, got {:?}", hit.effect());
            };
            assert_eq!(to, chain_sync.name());
            assert_eq!(
                msg.cast_ref::<HandlerMessage>().expect("HandlerMessage"),
                &HandlerMessage::Registered(PROTO_TEST.erase())
            );
        }
        running.interpret_breakpoint();
        running.enqueue_msg(&mux, [MuxMessage::WantNext(PROTO_TEST.erase())]);
        running.run(Run::skip_wakeups()).assert_busy([&reader]);

        // send a message towards the network
        let send_msg = |running: &mut SimulationRunning,
                        id: u64,
                        msg: u8,
                        len: usize,
                        proto_id: ProtocolId<Initiator>| {
            let bytes = vec![msg; len];
            let sent = StageRef::named_for_tests(&format!("sent_{id}"));
            running.enqueue_msg(
                &mux,
                [MuxMessage::Send(proto_id.erase(), Bytes::copy_from_slice(&bytes).try_into().unwrap(), sent.clone())],
            );
            sent
        };

        let assert_send = |running: &mut SimulationRunning, data: &[(usize, u8)], proto_id: ProtocolId<Initiator>| {
            running.run(Run::skip_wakeups()).assert_breakpoint("send");
            {
                let hit = running.breakpoint_effect();
                external::<SendEffect>(hit.effect()).assert_frame(conn_id, proto_id.erase(), data);
            }
        };
        let resume_send = |running: &mut SimulationRunning| {
            running.discard_breakpoint();
            running.complete_external(&writer, Ok::<(), SendError>(()));
        };
        let assert_and_resume_send =
            |running: &mut SimulationRunning, data: &[(usize, u8)], proto_id: ProtocolId<Initiator>| {
                assert_send(running, data, proto_id);
                resume_send(running);
            };
        let assert_respond = |running: &mut SimulationRunning, sent: &StageRef<Sent>| {
            running.run(Run::skip_wakeups()).assert_breakpoint("mux");
            {
                let hit = running.breakpoint_effect();
                let Effect::Send { to, msg, .. } = hit.effect() else {
                    panic!("expected send, got {:?}", hit.effect());
                };
                assert_eq!(to, sent.name());
                assert_eq!(msg.cast_ref::<Sent>().expect("Sent"), &Sent);
            }
            running.interpret_breakpoint();
        };

        // start write but don't let the writer finish yet
        let cr1 = send_msg(running, 101, 1, 1024, PROTO_TEST);
        assert_respond(running, &cr1);
        assert_send(running, &[(1024, 1)], PROTO_TEST);

        // put 1024 bytes into the proto buffer
        let cr2 = send_msg(running, 102, 2, 1024, PROTO_TEST);
        // put 10 bytes into the proto buffer
        let cr3 = send_msg(running, 103, 3, 10, PROTO_TEST);
        // the above are for checking correct responses via the CallRefs

        // fill segments for other two protocols
        let cr4 = send_msg(running, 104, 4, 66000, PROTO_HANDSHAKE);
        let cr5 = send_msg(running, 105, 5, 66000, PROTO_N2N_BLOCK_FETCH);

        resume_send(running);
        assert_and_resume_send(running, &[(65535, 5)], PROTO_N2N_BLOCK_FETCH);
        assert_and_resume_send(running, &[(65535, 4)], PROTO_HANDSHAKE);
        assert_respond(running, &cr2);
        assert_respond(running, &cr3);
        assert_and_resume_send(running, &[(1024, 2), (10, 3)], PROTO_TEST);
        assert_respond(running, &cr5);
        assert_and_resume_send(running, &[(465, 5)], PROTO_N2N_BLOCK_FETCH);
        assert_respond(running, &cr4);
        assert_and_resume_send(running, &[(465, 4)], PROTO_HANDSHAKE);

        let recv_header = RecvEffect::leading_edge(conn_id);
        let recv_header_rest = RecvEffect::assembly(conn_id, HEADER_REST, SDU_TIMEOUT_HANDSHAKE);
        let recv_msg =
            |running: &mut SimulationRunning, proto_id: ProtocolId<Responder>, bytes: &[u8], recv: &[&[u8]]| {
                let mut msg = Header::encode(proto_id, bytes, Timestamp::now()).into_inner();
                running.discard_breakpoint();
                running.complete_external(
                    &reader,
                    Ok::<NonEmptyBytes, ReceiveError>(msg.split_to(HEADER_LEADING_EDGE.get()).try_into().unwrap()),
                );
                running.run(Run::skip_wakeups()).assert_breakpoint("recv");
                {
                    let hit = running.breakpoint_effect();
                    assert_eq!(external::<RecvEffect>(hit.effect()), &recv_header_rest);
                }
                running.discard_breakpoint();
                running.complete_external(
                    &reader,
                    Ok::<NonEmptyBytes, ReceiveError>(msg.split_to(HEADER_REST.get()).try_into().unwrap()),
                );
                let msg = NonEmptyBytes::new(msg).unwrap();
                running.run(Run::skip_wakeups()).assert_breakpoint("recv");
                {
                    let hit = running.breakpoint_effect();
                    assert_eq!(
                        external::<RecvEffect>(hit.effect()),
                        &RecvEffect::assembly(conn_id, msg.len(), SDU_TIMEOUT_HANDSHAKE)
                    );
                }
                running.discard_breakpoint();
                running.complete_external(&reader, Ok::<NonEmptyBytes, ReceiveError>(msg));
                for recv in recv {
                    if recv.is_empty() {
                        running.run(Run::skip_wakeups()).assert_breakpoint("recv");
                        {
                            let hit = running.breakpoint_effect();
                            assert_eq!(external::<RecvEffect>(hit.effect()), &recv_header);
                        }
                        continue;
                    }
                    running.run(Run::skip_wakeups()).assert_breakpoint("mux");
                    {
                        let hit = running.breakpoint_effect();
                        let Effect::Send { to, msg, .. } = hit.effect() else {
                            panic!("expected send, got {:?}", hit.effect());
                        };
                        assert_eq!(to, chain_sync.name());
                        assert_eq!(
                            msg.cast_ref::<HandlerMessage>().expect("HandlerMessage"),
                            &HandlerMessage::FromNetwork(NonEmptyBytes::from_slice(recv).unwrap())
                        );
                    }
                    running.interpret_breakpoint();
                    running.enqueue_msg(&mux, [MuxMessage::WantNext(proto_id.initiator().erase())]);
                }
            };

        // send CBOR 1 followed by incomplete CBOR; "recv" effect always happens second
        recv_msg(running, PROTO_TEST.responder(), &[1, 24], &[&[1], &[]]);
        // send CBOR 25 continuation followed by CBOR 3
        recv_msg(running, PROTO_TEST.responder(), &[25, 3], &[&[24, 25], &[], &[3]]);

        // test buffer size violation
        recv_msg(running, PROTO_HANDSHAKE.responder(), &[1, 2, 3], &[]);
        running.run(Run::skip_wakeups()).assert_terminated(mux.name());

        drop_guard.defuse();
    }

    #[test]
    fn test_sdu_timeout_after_leading_edge() {
        let _guard = amaru_pure_stage::register_data_deserializer::<MuxMessage>();
        let _guard = amaru_pure_stage::register_effect_deserializer::<RecvEffect>();
        let _guard = amaru_pure_stage::register_data_deserializer::<State>();
        let _guard = amaru_pure_stage::register_data_deserializer::<Peer>();
        let _guard = amaru_pure_stage::register_data_deserializer::<(ConnectionId, StageRef<MuxMessage>, Role, Peer)>();

        let trace_buffer = TraceBuffer::new_shared(100, 1_000_000);
        let drop_guard = TraceBuffer::drop_guard(&trace_buffer);
        let mut network = SimulationBuilder::default().with_trace_buffer(trace_buffer);
        let mux = network.stage("mux", super::stage);
        let conn_id = ConnectionId::initial();
        let mux =
            network.wire_up(mux, State::new(conn_id, &[(PROTO_TEST.erase(), 1024)], Role::Initiator, test_peer()));

        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut running = network.run(rt.handle());
        running.breakpoint("recv", |eff| matches!(eff, Effect::External { effect, .. } if effect.is::<RecvEffect>()));
        running.breakpoint("spawn", |eff| matches!(eff, Effect::WireStage { .. }));

        running.enqueue_msg(
            &mux,
            [MuxMessage::Register {
                protocol: PROTO_TEST.erase(),
                frame: Frame::OneCborItem,
                handler: StageRef::named_for_tests("handler"),
                max_buffer: 1024,
            }],
        );
        running.run(Run::skip_wakeups()).assert_breakpoint("spawn");
        let _writer = wire_child_name(&running, mux.name(), (conn_id, (*mux).clone(), Role::Initiator, test_peer()));
        running.run(Run::skip_wakeups()).assert_breakpoint("spawn");
        let reader = wire_child_name(&running, mux.name(), (conn_id, (*mux).clone(), Role::Initiator, test_peer()));

        running.run(Run::skip_wakeups()).assert_breakpoint("recv");
        assert_eq!(external::<RecvEffect>(running.breakpoint_effect().effect()), &RecvEffect::leading_edge(conn_id));
        running.discard_breakpoint();
        running.complete_external(
            &reader,
            Ok::<NonEmptyBytes, ReceiveError>(Bytes::copy_from_slice(&[0]).try_into().unwrap()),
        );

        running.run(Run::skip_wakeups()).assert_breakpoint("recv");
        assert_eq!(
            external::<RecvEffect>(running.breakpoint_effect().effect()),
            &RecvEffect::assembly(conn_id, HEADER_REST, SDU_TIMEOUT_HANDSHAKE)
        );
        running.discard_breakpoint();
        running.complete_external(
            &reader,
            Err::<NonEmptyBytes, ReceiveError>(ReceiveError::sdu_timeout(conn_id, SDU_TIMEOUT_HANDSHAKE)),
        );
        running.run(Run::skip_wakeups()).assert_terminated(mux.name());
        drop_guard.defuse();
    }

    trait AssertBytes {
        fn assert_frame(&self, conn: ConnectionId, proto_id: ProtocolId<Erased>, data: &[(usize, u8)]);
    }
    impl AssertBytes for SendEffect {
        fn assert_frame(&self, conn: ConnectionId, proto_id: ProtocolId<Erased>, data: &[(usize, u8)]) {
            assert_eq!(self.conn, conn);
            let mut header = self.data.slice(..HEADER_LEN.get());
            let header = Header::decode(&mut header).unwrap().unwrap();
            assert_eq!(header.proto_id, proto_id);
            assert_eq!(header.length.get() as usize, data.iter().map(|(len, _)| len).sum::<usize>());
            let mut bytes = self.data.slice(HEADER_LEN.get()..);
            for &(len, msg) in data {
                assert_eq!(&bytes.split_to(len), &vec![msg; len]);
            }
        }
    }

    #[tokio::test]
    async fn test_tokio() {
        let _guard = amaru_pure_stage::register_data_deserializer::<MuxMessage>();
        let _guard = amaru_pure_stage::register_data_deserializer::<NonEmptyBytes>();
        let _guard = amaru_pure_stage::register_effect_deserializer::<SendEffect>();
        let _guard = amaru_pure_stage::register_effect_deserializer::<RecvEffect>();
        let _guard = amaru_pure_stage::register_data_deserializer::<State>();

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let server_addr = listener.local_addr().unwrap();
        let server_task = tokio::spawn(async move { listener.accept().await.unwrap().0 });

        let network = TokioConnections::new(65536);
        let conn_id = t(network.connect(Peer::try_from(server_addr).unwrap(), Duration::from_secs(5))).await.unwrap();
        let mut tcp = t(server_task).await.unwrap();

        let trace_buffer = TraceBuffer::new_shared(1000, 1000000);
        let trace_guard = TraceBuffer::drop_guard(&trace_buffer);
        let mut graph = TokioBuilder::default().with_trace_buffer(trace_buffer);

        let mux = graph.stage("mux", super::stage);
        let mux = graph.wire_up(mux, State::new(conn_id, &[(PROTO_TEST.erase(), 0)], Role::Initiator, test_peer()));

        let (output, mut rx) = graph.output::<HandlerMessage>("output", 10);
        let (sent, mut sent_rx) = graph.output::<Sent>("sent", 10);
        let input = graph.input(&mux);

        graph.resources().put::<ConnectionsResource>(Arc::new(network));

        let running = graph.run(Handle::current());

        input
            .send(MuxMessage::Send(PROTO_TEST.erase(), Bytes::copy_from_slice(&[1, 24, 33]).try_into().unwrap(), sent))
            .await
            .unwrap();
        let mut buf = [0u8; 11];
        assert_eq!(t(tcp.read_exact(&mut buf)).await.unwrap(), 11);
        t(sent_rx.next()).await.unwrap();
        // first four bytes are timestamp; proto ID is 257 (0x0101), length is 3
        assert_eq!(&buf[4..], [1, 1, 0, 3, 1, 24, 33]);

        input
            .send(MuxMessage::Register {
                protocol: PROTO_TEST.erase(),
                frame: Frame::OneCborItem,
                handler: output,
                max_buffer: 100,
            })
            .await
            .unwrap();
        assert_eq!(t(rx.next()).await.unwrap(), HandlerMessage::Registered(PROTO_TEST.erase()));

        input.send(MuxMessage::WantNext(PROTO_TEST.erase())).await.unwrap();

        // need to flip role bit before sending as responses
        buf[4] |= 0x80;

        t(tcp.write_all(&buf)).await.unwrap();
        t(tcp.flush()).await.unwrap();
        assert_eq!(t(rx.next()).await.unwrap(), HandlerMessage::FromNetwork(NonEmptyBytes::from_slice(&[1]).unwrap()));
        s(rx.next()).await;
        input.send(MuxMessage::WantNext(PROTO_TEST.erase())).await.unwrap();
        assert_eq!(
            t(rx.next()).await.unwrap(),
            HandlerMessage::FromNetwork(NonEmptyBytes::from_slice(&[24, 33]).unwrap())
        );

        // wrong protocol ID
        buf[5] += 1;
        t(tcp.write_all(&buf)).await.unwrap();
        t(tcp.flush()).await.unwrap();
        t(running.join()).await;

        trace_guard.defuse();
    }
}
