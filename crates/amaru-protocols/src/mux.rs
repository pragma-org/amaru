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

use amaru_kernel::{NonEmptyBytes, Peer, cbor};
use amaru_observability::{Instrument, debug, debug_span, error, info, trace, warn};
use amaru_ouroboros::ConnectionId;
use amaru_pure_stage::{Effects, Instant, OrTerminateWith, SendData, StageRef, TryInStage, TrySend, Void};
use anyhow::Context;
use bytes::{Buf, BufMut, Bytes, BytesMut, TryGetError};

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

pub(crate) const MAX_SEGMENT_SIZE: usize = 65535;

/// Mux SDU assembly/send timer during the first Handshake on a bearer.
pub const SDU_TIMEOUT_HANDSHAKE: Duration = Duration::from_secs(10);
/// Mux SDU assembly/send timer after that Handshake has finished.
pub const SDU_TIMEOUT_ESTABLISHED: Duration = Duration::from_secs(30);

/// Bulk mailbox of the mux stage.
///
/// A hot duplex connection runs up to ten handlers. One `Send` and one `WantNext`
/// from each, plus `FromNetwork` and `Written`, is 22; 24 leaves room for a
/// `Register` or `SetSduTimeout` in the same burst.
pub const MUX_MAILBOX_SIZE: usize = 24;

/// How long a handler mailbox may stay full before this connection is closed.
///
/// The stall is local queueing, not the peer's network or agency time, so every
/// protocol waits the same. The retry fires once a second; five seconds is a few
/// of those retries, then a handler that has stopped reading faults the connection.
/// The peer is not scored as adversarial.
pub const INGRESS_DEADLINE: Duration = Duration::from_secs(5);

/// One coalesced retry for ingress the handler mailbox did not accept.
///
/// Slot 0 is the default timeout ([`amaru_pure_stage::Effects::set_timeout`]).
/// Ingress uses slot 1 and egress uses slot 2, so that default cannot replace
/// either timer and the two retries do not share a slot.
const INGRESS_RETRY_SLOT: u64 = 1;

/// One coalesced retry when the writer returned [`TrySend::Full`].
///
/// Bytes still waiting for free space in the egress buffer are copied in the
/// transition that hands a segment to the writer, not on this timer.
const EGRESS_RETRY_SLOT: u64 = 2;

/// Copy pending bytes into `outgoing` until it holds one segment.
///
/// A message that does not fit is split. [`Sent`] for that message is returned
/// only once its last byte has been copied. `outgoing` never grows past
/// [`MAX_SEGMENT_SIZE`].
fn fill_egress(outgoing: &mut BytesMut, pending: &mut VecDeque<DeferredSend>) -> Vec<StageRef<Sent>> {
    let mut done = Vec::new();
    while outgoing.len() < MAX_SEGMENT_SIZE {
        let (chunk, finished) = {
            let Some(front) = pending.front_mut() else {
                break;
            };
            let room = MAX_SEGMENT_SIZE - outgoing.len();
            let n = room.min(front.bytes.len());
            if n == 0 {
                break;
            }
            let chunk = front.bytes.split_to(n);
            (chunk, front.bytes.is_empty())
        };
        outgoing.extend_from_slice(&chunk);
        if finished && let Some(item) = pending.pop_front() {
            done.push(item.sent);
        }
    }
    debug_assert!(outgoing.len() <= MAX_SEGMENT_SIZE);
    done
}

const HEADER_LEADING_EDGE: NonZeroUsize = NonZeroUsize::MIN;
const HEADER_REST: NonZeroUsize = const {
    let ret = NonZeroUsize::new(7).expect("non-zero");
    assert!(matches!(HEADER_LEADING_EDGE.checked_add(ret.get()), Some(HEADER_LEN)));
    ret
};

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
    /// Each message is a single CBOR item.
    ///
    /// Framing only measures that item's byte length. Nested structure is skipped iteratively, so a
    /// deeply nested payload cannot overflow the stack at the mux layer.
    OneCborItem,
    /// No message parsing, just buffer the data
    Buffer,
}

impl Frame {
    /// Pull one complete message off `data` according to this framing policy.
    ///
    /// `OneCborItem` uses minicbor's iterative skip. An incomplete item returns `Ok(None)` and
    /// leaves `data` unchanged.
    pub fn try_consume(&self, data: &mut BytesMut) -> Result<Option<NonEmptyBytes>, cbor::decode::Error> {
        let Some(len) = self.peek_len(data)? else {
            return Ok(None);
        };
        Ok(Some(take_frame(data, len)))
    }

    /// Byte length of the next complete message, without removing it.
    ///
    /// `Ok(None)` means the buffer does not yet hold a whole message.
    pub fn peek_len(&self, data: &[u8]) -> Result<Option<usize>, cbor::decode::Error> {
        match self {
            Frame::OneCborItem => {
                let mut decoder = cbor::Decoder::new(data);
                match decoder.skip() {
                    Ok(()) => Ok(Some(decoder.position())),
                    Err(e) if e.is_end_of_input() => Ok(None),
                    Err(e) => Err(e),
                }
            }
            Frame::Buffer => Ok(None),
        }
    }
}

fn take_frame(data: &mut BytesMut, len: usize) -> NonEmptyBytes {
    let item = data.copy_to_bytes(len);
    #[expect(clippy::expect_used)]
    item.try_into().expect("frame length is non-zero")
}

#[cfg(test)]
mod one_cbor_item_tests {
    use std::thread;

    use test_case::test_case;

    use super::*;

    /// The stack a spawned thread gets by default, and therefore what a connection's worker frames
    /// inbound bytes on. `.cargo/config.toml` raises `RUST_MIN_STACK` for cargo-launched processes
    const DEFAULT_STACK: usize = 2 * 1024 * 1024;

    fn consume(bytes: &[u8]) -> Result<(Option<NonEmptyBytes>, BytesMut), cbor::decode::Error> {
        let mut data = BytesMut::from(bytes);
        let item = Frame::OneCborItem.try_consume(&mut data)?;
        Ok((item, data))
    }

    #[test]
    fn empty_is_incomplete() {
        let (item, rest) = consume(&[]).unwrap();
        assert_eq!(item, None);
        assert!(rest.is_empty());
    }

    #[test]
    fn peek_len_leaves_the_bytes_in_place() {
        let data = [0x01, 0x02];
        assert_eq!(Frame::OneCborItem.peek_len(&data).unwrap(), Some(1));
        assert_eq!(data, [0x01, 0x02]);
    }

    #[test]
    fn incomplete_array_waits() {
        let (item, rest) = consume(&[0x81]).unwrap();
        assert_eq!(item, None);
        assert_eq!(&rest[..], &[0x81]);
    }

    #[test]
    fn incomplete_bytes_wait() {
        let (item, rest) = consume(&[0x45, 0x01, 0x02]).unwrap();
        assert_eq!(item, None);
        assert_eq!(&rest[..], &[0x45, 0x01, 0x02]);
    }

    #[test]
    fn complete_int_leaves_the_next_item() {
        let (item, rest) = consume(&[0x01, 0x02]).unwrap();
        assert_eq!(item, Some(NonEmptyBytes::from_slice(&[0x01]).unwrap()));
        assert_eq!(&rest[..], &[0x02]);
    }

    #[test]
    fn nested_array_is_one_item() {
        let bytes = [0x81, 0x81, 0x00, 0x02];
        let (item, rest) = consume(&bytes).unwrap();
        assert_eq!(item.unwrap().as_ref(), &[0x81, 0x81, 0x00]);
        assert_eq!(&rest[..], &[0x02]);
    }

    #[test]
    fn indefinite_array_is_one_item() {
        let bytes = [0x9f, 0x01, 0xff, 0x02];
        let (item, rest) = consume(&bytes).unwrap();
        assert_eq!(item.unwrap().as_ref(), &[0x9f, 0x01, 0xff]);
        assert_eq!(&rest[..], &[0x02]);
    }

    #[test]
    fn tagged_byte_string_is_not_parsed_as_nested_cbor() {
        // Tag 24 wrapping a 1-byte string of `0xff`. The inner bytes are not a CBOR item; framing
        // still takes the complete outer item.
        let bytes = [0xd8, 24, 0x41, 0xff];
        let (item, rest) = consume(&bytes).unwrap();
        assert_eq!(item.unwrap().as_ref(), &bytes);
        assert!(rest.is_empty());
    }

    #[test]
    fn invalid_additional_info_is_an_error() {
        assert!(consume(&[0x1c]).is_err());
    }

    /// A regression aborts the test binary rather than reporting a failure.
    #[test_case(&[0x81], &[]; "definite arrays")]
    #[test_case(&[0x9f], &[0xff]; "indefinite arrays")]
    #[test_case(&[0xa1, 0x00], &[]; "definite maps")]
    #[test_case(&[0xbf, 0x00], &[0xff]; "indefinite maps")]
    fn deep_nesting_does_not_overflow_the_stack(open: &'static [u8], close: &'static [u8]) {
        const DEPTH: usize = 100_000;
        const LEAF: u8 = 0x00;

        let test = move || {
            let mut bytes = Vec::with_capacity((open.len() + close.len()) * DEPTH + 1);
            for _ in 0..DEPTH {
                bytes.extend_from_slice(open);
            }
            bytes.push(LEAF);
            for _ in 0..DEPTH {
                bytes.extend_from_slice(close);
            }

            let (item, rest) = consume(&bytes).unwrap();
            assert_eq!(item.unwrap().as_ref(), bytes.as_slice());
            assert!(rest.is_empty());
        };

        thread::Builder::new().stack_size(DEFAULT_STACK).spawn(test).unwrap().join().unwrap();
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum HandlerMessage {
    Registered(ProtocolId<Erased>),
    FromNetwork(NonEmptyBytes),
}

/// Mux citizen after an initiator has sent `MsgDone`. Any further frame is a protocol error.
async fn done_trap(peer: Peer, msg: HandlerMessage, eff: Effects<HandlerMessage>) -> Peer {
    match msg {
        HandlerMessage::Registered(_) => peer,
        HandlerMessage::FromNetwork(_) => {
            error!(
                protocols::mux::FAILED,
                peer,
                role = "trap",
                operation = "after_done",
                error = "frame after MsgDone"
            );
            return eff.terminate().await;
        }
    }
}

pub async fn install_done_trap<M: SendData>(
    muxer: &StageRef<MuxMessage>,
    protocol: ProtocolId<Erased>,
    peer: Peer,
    eff: &Effects<M>,
    tombstone: M,
) {
    let trap = eff.stage("done-trap", done_trap).await;
    let trap = eff.supervise(trap, tombstone);
    let trap = eff.wire_up(trap, peer).await;
    eff.send(
        muxer,
        MuxMessage::Register {
            protocol,
            frame: Frame::OneCborItem,
            handler: trap,
            max_buffer: crate::protocol::ingress_limit(protocol),
        },
    )
    .await;
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
    /// Send the given message on the protocol ID.
    ///
    /// [`Sent`] is delivered once the last byte of this message has been copied
    /// into the lane's egress buffer. That buffer holds at most one segment.
    /// Bytes already in it can belong to several messages,
    /// and one message can be split across segments. The rest of a message waits,
    /// in arrival order, until a segment leaves room. The mux does not block on
    /// the writer to answer this call.
    ///
    /// One handler runs on a lane and submits the next message only after this
    /// call returns, so the previous message's last byte is already in the buffer.
    /// Nothing queues ahead of a live call beyond that one buffer and the segment
    /// already on the wire.
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
    /// Retry ingress that stayed buffered because a handler mailbox was full.
    IngressRetry,
    /// Retry a segment the writer returned [`TrySend::Full`] for.
    ///
    /// Bytes waiting for room in the egress buffer are not woken by this timer.
    /// They are copied in the transition that hands a segment to the writer.
    EgressRetry,
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
    /// Create a new state with the given connection ID, buffering the given protocols.
    ///
    /// Upon receiving the first message, the stage starts reading from the network.
    /// Bytes for a protocol in `buffer` (or named later by [`MuxMessage::Buffer`]) are held
    /// until `Register` installs its handler. Bytes for any other protocol fail the connection.
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
            warn!(
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
            muxer.defer_send(proto_id, bytes.into(), sent);
            pump(muxer, sending, writer, eff).await?;
            muxer.sync_egress_retry(eff).await;
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
            pump(muxer, sending, writer, eff).await?;
            muxer.sync_egress_retry(eff).await;
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
        MuxMessage::IngressRetry => muxer.retry_ingress(eff).await,
        MuxMessage::EgressRetry => {
            muxer.egress_retry_armed = false;
            pump(muxer, sending, writer, eff).await?;
            muxer.sync_egress_retry(eff).await;
            Ok(())
        }
    }
}

/// Fill each lane's egress buffer, then hand at most one segment to the writer.
///
/// [`Sent`] is delivered here once a message's last byte has been copied into
/// that buffer. `TrySend::Queued` frees the buffer, and bytes that were waiting
/// are copied in this same transition. [`MuxMessage::EgressRetry`] is not used
/// for that wait.
///
/// `TrySend::Full` leaves the segment queued and does not set `sending`: the
/// one-outstanding-SDU invariant broke, and [`MuxMessage::EgressRetry`] tries
/// again. `TrySend::Gone` closes the connection. The peer is not scored.
async fn pump(
    muxer: &mut Muxer,
    sending: &mut bool,
    writer: &StageRef<OutgoingSdu>,
    eff: &Effects<MuxMessage>,
) -> anyhow::Result<()> {
    muxer.feed_egress(eff).await;
    if *sending {
        return Ok(());
    }
    if let Emit::Queued = muxer.try_emit(writer, eff).await? {
        *sending = true;
        muxer.feed_egress(eff).await;
    }
    Ok(())
}

enum Emit {
    /// One segment is in the writer mailbox. Its bytes have left unsent egress.
    Queued,
    /// Nothing is waiting to be written.
    Idle,
    /// The writer mailbox refused the segment. The bytes stay queued.
    Blocked,
}

async fn read_segment(
    (conn, muxer, role, peer): (ConnectionId, StageRef<MuxMessage>, Role, Peer),
    Read { sdu_timeout }: Read,
    eff: Effects<Read>,
) -> (ConnectionId, StageRef<MuxMessage>, Role, Peer) {
    // A modelled writer test holds the reader here so the simulation stays
    // Sleeping. Production builds do not compile this wait.
    #[cfg(test)]
    if let Some(hold) = crate::network_effects::modelled_link::reader_hold() {
        eff.wait(hold).await;
    }
    let header = loop {
        let first = Network::new(&eff)
            .recv(conn, HEADER_LEADING_EDGE, None)
            .or_terminate_with(&eff, async |err| {
                warn!(
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
pub(crate) const SEGMENT_HEADER_LEN: usize = 8;
const HEADER_LEN: NonZeroUsize = NonZeroUsize::new(SEGMENT_HEADER_LEN).expect("8 is a valid non-zero size");

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
    /// `INGRESS_RETRY_SLOT` is armed. Replaced only after it fires or is cleared.
    ingress_retry_armed: bool,
    /// `EGRESS_RETRY_SLOT` is armed.
    egress_retry_armed: bool,
    /// The writer returned [`TrySend::Full`] for a segment that is still queued.
    writer_blocked: bool,
}

impl Muxer {
    pub fn new(role: Role) -> Self {
        Self {
            protocols: Protocols::new(),
            outgoing: Vec::new(),
            next_out: 0,
            role,
            sdu_timeout: SDU_TIMEOUT_HANDSHAKE,
            ingress_retry_armed: false,
            egress_retry_armed: false,
            writer_blocked: false,
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

    pub async fn register(
        &mut self,
        proto_id: ProtocolId<Erased>,
        frame: Frame,
        max_buffer: usize,
        handler: StageRef<HandlerMessage>,
        eff: &Effects<MuxMessage>,
    ) -> anyhow::Result<()> {
        async {
            self.do_register(proto_id, frame, max_buffer, handler).registered_pending = true;
            self.flush_protocol(proto_id, eff).await?;
            self.sync_ingress_retry(eff).await;
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

    fn proto_mut(&mut self, proto_id: ProtocolId<Erased>) -> &mut PerProto {
        #[expect(clippy::expect_used)]
        self.protocols.get_mut(&proto_id).expect("protocol registered")
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

    /// Queue `bytes` behind anything already waiting on this protocol.
    ///
    /// [`feed_egress`](Self::feed_egress) copies what fits into the bounded buffer.
    fn defer_send(&mut self, proto_id: ProtocolId<Erased>, bytes: Bytes, sent: StageRef<Sent>) {
        let _span = debug_span!(
            protocols::mux::protocol::OUTGOING,
            proto_id = format!("{}", proto_id),
            bytes = bytes.len() as u64
        );
        let _guard = _span.enter();

        trace!(protocols::mux::protocol::ENQUEUE, proto_id = proto_id.to_string(), bytes = bytes.len() as u64);
        self.proto_mut(proto_id).deferred.push_back(DeferredSend { bytes, sent });
    }

    /// Copy waiting bytes into each lane's egress buffer, registration order.
    ///
    /// Delivers [`Sent`] once a message's last byte is in that buffer.
    async fn feed_egress(&mut self, eff: &Effects<MuxMessage>) {
        let order = self.outgoing.clone();
        for proto_id in order {
            let done = {
                let Some(proto) = self.protocols.get_mut(&proto_id) else {
                    continue;
                };
                fill_egress(&mut proto.outgoing, &mut proto.deferred)
            };
            for sent in done {
                eff.send(&sent, Sent).await;
            }
        }
    }

    /// Hand the next segment to the writer without blocking.
    ///
    /// Bytes leave `outgoing` only after [`TrySend::Queued`]. `Full` keeps them.
    /// `Gone` fails the mux.
    async fn try_emit(&mut self, writer: &StageRef<OutgoingSdu>, eff: &Effects<MuxMessage>) -> anyhow::Result<Emit> {
        let Some((idx, proto_id, bytes)) = self.peek_segment() else {
            self.writer_blocked = false;
            return Ok(Emit::Idle);
        };
        let header = self.encode_header(eff, proto_id, &bytes).await;
        match eff.try_send(writer, OutgoingSdu { data: header, timeout: self.sdu_timeout }).await {
            TrySend::Queued => {
                self.commit_segment(proto_id, bytes.len());
                self.next_out = (idx + 1) % self.outgoing.len();
                self.writer_blocked = false;
                trace!(
                    protocols::mux::protocol::SEGMENT_SENT,
                    proto_id = proto_id.to_string(),
                    bytes = bytes.len() as u64,
                    next = self.next_out as u64
                );
                Ok(Emit::Queued)
            }
            TrySend::Full => {
                self.writer_blocked = true;
                Ok(Emit::Blocked)
            }
            TrySend::Gone => {
                anyhow::bail!(
                    "writer gone while sending protocol {proto_id}; segment stays queued; peer is not treated as adversarial"
                )
            }
        }
    }

    fn peek_segment(&self) -> Option<(usize, ProtocolId<Erased>, Bytes)> {
        if self.outgoing.is_empty() {
            return None;
        }
        for idx in (self.next_out..self.outgoing.len()).chain(0..self.next_out) {
            let proto_id = self.outgoing[idx];
            let Some(proto) = self.protocols.get(&proto_id) else {
                continue;
            };
            if proto.outgoing.is_empty() {
                continue;
            }
            let size = proto.outgoing.len().min(MAX_SEGMENT_SIZE);
            let bytes = Bytes::copy_from_slice(&proto.outgoing[..size]);
            return Some((idx, proto_id, bytes));
        }
        None
    }

    fn commit_segment(&mut self, proto_id: ProtocolId<Erased>, size: usize) {
        let proto = self.proto_mut(proto_id);
        let _ = proto.outgoing.split_to(size);
    }

    async fn sync_egress_retry(&mut self, eff: &Effects<MuxMessage>) {
        // Waiting bytes are copied when a segment is handed off, not on a timer.
        let need = self.writer_blocked;
        if need && !self.egress_retry_armed {
            eff.set_timeout_at(EGRESS_RETRY_SLOT, crate::protocol::NETWORK_SEND_TIMEOUT, MuxMessage::EgressRetry).await;
            self.egress_retry_armed = true;
        } else if !need && self.egress_retry_armed {
            eff.clear_timeout_at(EGRESS_RETRY_SLOT).await;
            self.egress_retry_armed = false;
        }
    }

    pub async fn received(
        &mut self,
        _timestamp: Timestamp,
        proto_id: ProtocolId<Erased>,
        bytes: Bytes,
        eff: &Effects<MuxMessage>,
    ) -> anyhow::Result<()> {
        let byte_len = bytes.len() as u64;
        async {
            {
                let Some(proto) = self.protocols.get_mut(&proto_id) else {
                    anyhow::bail!("received data for unknown protocol {}", proto_id);
                };
                if proto.max_buffer == 0 {
                    debug!(protocols::mux::protocol::IGNORING_BYTES, bytes = bytes.len());
                    return Ok(());
                }
                trace!(protocols::mux::protocol::BYTES_RECEIVED, wanted = proto.wanted);
                if proto.incoming.len() + bytes.len() > proto.max_buffer {
                    info!(
                        protocols::mux::protocol::BUFFER_EXCEEDED,
                        buffered = proto.incoming.len(),
                        max_buffer = proto.max_buffer
                    );
                    anyhow::bail!(
                        "message (size {}) plus buffer (size {}) exceeds limit ({})",
                        bytes.len(),
                        proto.incoming.len(),
                        proto.max_buffer
                    );
                }
                proto.incoming.extend(&bytes);
            }
            self.flush_protocol(proto_id, eff).await?;
            self.sync_ingress_retry(eff).await;
            Ok(())
        }
        .instrument(debug_span!(protocols::mux::protocol::RECEIVED, bytes = byte_len))
        .await
    }

    pub async fn want_next(&mut self, proto_id: ProtocolId<Erased>, eff: &Effects<MuxMessage>) -> anyhow::Result<()> {
        async {
            let buffered = {
                #[allow(clippy::expect_used)]
                let proto = self
                    .protocols
                    .get_mut(&proto_id)
                    .ok_or_else(|| anyhow::anyhow!("protocol {} not registered", proto_id))
                    .expect("internal error");
                trace!(protocols::mux::protocol::BYTES_RECEIVED, wanted = proto.wanted);
                let buffered = proto.frame.peek_len(&proto.incoming)?;
                proto.wanted += 1;
                buffered
            };
            if buffered.is_none() {
                trace!(protocols::mux::protocol::DELIVERY_DEFERRED);
            }
            self.flush_protocol(proto_id, eff).await?;
            self.sync_ingress_retry(eff).await;
            Ok(())
        }
        .instrument(debug_span!(protocols::mux::protocol::WANT_NEXT,))
        .await
    }

    /// Deliver deferred ingress in registration order.
    ///
    /// The timer that scheduled this wakeup has fired, so it is no longer armed. A protocol
    /// whose frame is still deferred once [`INGRESS_DEADLINE`] has elapsed closes the connection.
    /// That is a local handler that stopped reading. The peer is not treated as adversarial:
    /// nothing is scored, and no other connection is involved.
    ///
    /// `TrySend::Gone` drops the message and continues. The handler stage is already gone, which
    /// is the same observation as a failed `send` today; the bytes will not be admitted later,
    /// so they are removed instead of held until the buffer limit faults the bearer.
    async fn retry_ingress(&mut self, eff: &Effects<MuxMessage>) -> anyhow::Result<()> {
        self.ingress_retry_armed = false;
        let now = eff.clock().await;
        let order = self.outgoing.clone();
        for proto_id in order {
            self.flush_protocol(proto_id, eff).await?;
        }
        self.fault_expired(now)?;
        self.sync_ingress_retry(eff).await;
        Ok(())
    }

    fn fault_expired(&self, now: Instant) -> anyhow::Result<()> {
        for proto_id in &self.outgoing {
            let Some(proto) = self.protocols.get(proto_id) else {
                continue;
            };
            let Some(since) = proto.deferred_since else {
                continue;
            };
            if now.saturating_since(since) >= INGRESS_DEADLINE {
                anyhow::bail!(
                    "ingress deferred past deadline for protocol {proto_id}; handler did not accept buffered data; peer is not treated as adversarial"
                );
            }
        }
        Ok(())
    }

    async fn sync_ingress_retry(&mut self, eff: &Effects<MuxMessage>) {
        let deferred =
            self.outgoing.iter().any(|id| self.protocols.get(id).is_some_and(|pp| pp.deferred_since.is_some()));
        if deferred && !self.ingress_retry_armed {
            eff.set_timeout_at(INGRESS_RETRY_SLOT, crate::protocol::NETWORK_SEND_TIMEOUT, MuxMessage::IngressRetry)
                .await;
            self.ingress_retry_armed = true;
        } else if !deferred && self.ingress_retry_armed {
            eff.clear_timeout_at(INGRESS_RETRY_SLOT).await;
            self.ingress_retry_armed = false;
        }
    }

    async fn flush_protocol(&mut self, proto_id: ProtocolId<Erased>, eff: &Effects<MuxMessage>) -> anyhow::Result<()> {
        loop {
            let Some(proto) = self.protocols.get_mut(&proto_id) else {
                return Ok(());
            };
            if proto.registered_pending {
                let handler = proto.handler.clone();
                proto.registered_pending = false;
                match eff.try_send(&handler, HandlerMessage::Registered(proto_id)).await {
                    TrySend::Queued => {}
                    TrySend::Full => {
                        self.proto_mut(proto_id).registered_pending = true;
                        self.note_deferred(proto_id, eff).await;
                        return Ok(());
                    }
                    // The handler is gone. The notice was not queued. A dead handler is already
                    // the supervisor's event, so this does not fault the connection. The next
                    // iteration drops any buffered frame the same way.
                    TrySend::Gone => {}
                }
                continue;
            }

            if proto.wanted == 0 {
                proto.deferred_since = None;
                return Ok(());
            }
            let Some(len) = proto.frame.peek_len(&proto.incoming)? else {
                proto.deferred_since = None;
                return Ok(());
            };
            let mut prefix = proto.incoming.split_to(len);
            proto.wanted -= 1;
            let handler = proto.handler.clone();
            #[expect(clippy::expect_used)]
            let frame = NonEmptyBytes::from_slice(&prefix).expect("peeked length is non-empty");
            match eff.try_send(&handler, HandlerMessage::FromNetwork(frame)).await {
                TrySend::Queued => {
                    trace!(protocols::mux::protocol::MESSAGE_EXTRACTED, bytes = prefix.len());
                }
                TrySend::Full => {
                    {
                        let proto = self.proto_mut(proto_id);
                        let suffix = std::mem::take(&mut proto.incoming);
                        prefix.unsplit(suffix);
                        proto.incoming = prefix;
                        proto.wanted += 1;
                    }
                    self.note_deferred(proto_id, eff).await;
                    return Ok(());
                }
                // The frame was already taken off the buffer. A later iteration clears the
                // deferral once nothing complete remains. A dead handler is not a fault.
                TrySend::Gone => {}
            }
        }
    }

    async fn note_deferred(&mut self, proto_id: ProtocolId<Erased>, eff: &Effects<MuxMessage>) {
        if self.protocols.get(&proto_id).is_some_and(|pp| pp.deferred_since.is_some()) {
            return;
        }
        let now = eff.clock().await;
        self.proto_mut(proto_id).deferred_since = Some(now);
    }
}

#[derive(PartialEq, serde::Serialize, serde::Deserialize)]
struct PerProto {
    incoming: BytesMut,
    outgoing: BytesMut,
    /// Bytes not yet copied into [`Self::outgoing`], in arrival order.
    ///
    /// A front message may already have had a prefix copied. [`Sent`] stays here
    /// until that message's last byte is in the buffer.
    deferred: VecDeque<DeferredSend>,
    handler: StageRef<HandlerMessage>,
    wanted: usize,
    frame: Frame,
    max_buffer: usize,
    /// `Registered` has not been admitted to `handler` yet.
    registered_pending: bool,
    /// When the current mailbox deferral started. `None` when nothing is waiting on the handler.
    deferred_since: Option<Instant>,
}

#[derive(PartialEq, serde::Serialize, serde::Deserialize)]
struct DeferredSend {
    bytes: Bytes,
    sent: StageRef<Sent>,
}

impl std::fmt::Debug for PerProto {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PerProto")
            .field("incoming", &self.incoming.len())
            .field("outgoing", &self.outgoing.len())
            .field("deferred", &self.deferred.len())
            .field("handler", &self.handler)
            .field("wanted", &self.wanted)
            .field("frame", &self.frame)
            .field("max_buffer", &self.max_buffer)
            .field("registered_pending", &self.registered_pending)
            .field("deferred_since", &self.deferred_since)
            .finish()
    }
}

impl PerProto {
    pub fn new(handler: StageRef<HandlerMessage>, frame: Frame, max_buffer: usize) -> Self {
        Self {
            incoming: BytesMut::new(),
            outgoing: BytesMut::new(),
            deferred: VecDeque::new(),
            handler,
            wanted: 0,
            frame,
            max_buffer,
            registered_pending: false,
            deferred_since: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{fmt, sync::Arc, time::Duration};

    use amaru_network::connection::TokioConnections;
    use amaru_ouroboros::ConnectionsResource;
    use amaru_ouroboros_traits::ConnectionProvider;
    use amaru_pure_stage::{
        CallAdmission, Effect, ExternalEffect, Name, StageGraph, TraceMatch, TrySend,
        simulation::{Blocked, Run, SimulationBuilder, SimulationRunning, running::OverrideResult},
        stage_ref::StageStateRef,
        tokio::TokioBuilder,
        trace_buffer::{TraceBuffer, TraceEntry},
        trace_match::{assert_trace_contains, tm_resume_try_send, tm_try_send, tm_try_send_type},
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
        protocol::{
            Initiator, MIN_PEER_BANDWIDTH_BPS, PROTO_HANDSHAKE, PROTO_N2N_BLOCK_FETCH, PROTO_TEST, Responder,
            egress_admission_deadline, egress_buffer_drain,
        },
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
            running.breakpoint("mux", move |eff| {
                matches!(
                    eff,
                    Effect::Send { from, to, .. } | Effect::TrySend { from, to, .. }
                        if from == &mux_name && to != &writer && to != &reader
                )
            });
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
            let Effect::TrySend { to, msg, .. } = hit.effect() else {
                panic!("expected try_send, got {:?}", hit.effect());
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
        // Messages that fit are `Sent` while the writer is busy. A message larger than one
        // segment is `Sent` only after the handoff that frees room for its last byte.
        assert_respond(running, &cr2);
        assert_respond(running, &cr3);
        assert_and_resume_send(running, &[(65535, 5)], PROTO_N2N_BLOCK_FETCH);
        assert_respond(running, &cr5);
        assert_and_resume_send(running, &[(65535, 4)], PROTO_HANDSHAKE);
        assert_respond(running, &cr4);
        assert_and_resume_send(running, &[(1024, 2), (10, 3)], PROTO_TEST);
        assert_and_resume_send(running, &[(465, 5)], PROTO_N2N_BLOCK_FETCH);
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
                        let Effect::TrySend { to, msg, .. } = hit.effect() else {
                            panic!("expected try_send, got {:?}", hit.effect());
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

    fn with_buffered_mux(test: impl FnOnce(&mut SimulationRunning, &StageRef<MuxMessage>)) {
        let _ = tracing_subscriber::fmt().with_env_filter(EnvFilter::from_default_env()).with_test_writer().try_init();
        let _mux_msg = amaru_pure_stage::register_data_deserializer::<MuxMessage>();
        let _bytes = amaru_pure_stage::register_data_deserializer::<NonEmptyBytes>();
        let _handler_msg = amaru_pure_stage::register_data_deserializer::<HandlerMessage>();
        let _state = amaru_pure_stage::register_data_deserializer::<State>();
        let _peer = amaru_pure_stage::register_data_deserializer::<Peer>();
        let _reader_state =
            amaru_pure_stage::register_data_deserializer::<(ConnectionId, StageRef<MuxMessage>, Role, Peer)>();

        let mut network = SimulationBuilder::default();
        let mux = network.stage("mux", super::stage);
        let conn_id = ConnectionId::initial();
        let mux =
            network.wire_up(mux, State::new(conn_id, &[(PROTO_TEST.erase(), 1024)], Role::Initiator, test_peer()));
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut running = network.run(rt.handle());
        running.breakpoint("recv", |eff| matches!(eff, Effect::External { effect, .. } if effect.is::<RecvEffect>()));
        let handler_name = StageRef::<HandlerMessage>::named_for_tests("handler").name().clone();
        running.breakpoint("to-handler", move |eff| matches!(eff, Effect::TrySend { to, .. } if to == &handler_name));
        test(&mut running, &mux);
    }

    fn assert_to_handler(running: &SimulationRunning, expected: &HandlerMessage) {
        let hit = running.breakpoint_effect();
        let Effect::TrySend { msg, .. } = hit.effect() else {
            panic!("expected try_send, got {:?}", hit.effect());
        };
        assert_eq!(msg.cast_ref::<HandlerMessage>().expect("HandlerMessage"), expected);
    }

    /// Skip reader recv suspensions until the mux sends to the handler or stops.
    fn run_until_handler(running: &mut SimulationRunning) -> Blocked {
        for _ in 0..8 {
            let blocked = running.run(Run::skip_wakeups());
            if matches!(&blocked, Blocked::Breakpoint(name) if name.as_str() == "recv") {
                running.discard_breakpoint();
                continue;
            }
            return blocked;
        }
        panic!("mux made no progress");
    }

    #[test]
    fn buffered_protocol_holds_segment_until_register() {
        with_buffered_mux(|running, mux| {
            let payload = NonEmptyBytes::from_slice(&[0x18, 0x2a]).unwrap();
            // Wire id is the peer's view. `received` looks up the opposite, which is the buffered id.
            running.enqueue_msg(
                mux,
                [
                    MuxMessage::FromNetwork(Timestamp(1), PROTO_TEST.opposite().erase(), payload.clone()),
                    MuxMessage::Register {
                        protocol: PROTO_TEST.erase(),
                        frame: Frame::OneCborItem,
                        handler: StageRef::named_for_tests("handler"),
                        max_buffer: 1024,
                    },
                ],
            );

            assert!(matches!(run_until_handler(running), Blocked::Breakpoint(name) if name.as_str() == "to-handler"));
            assert_to_handler(running, &HandlerMessage::Registered(PROTO_TEST.erase()));
            running.interpret_breakpoint();

            running.enqueue_msg(mux, [MuxMessage::WantNext(PROTO_TEST.erase())]);
            assert!(matches!(run_until_handler(running), Blocked::Breakpoint(name) if name.as_str() == "to-handler"));
            assert_to_handler(running, &HandlerMessage::FromNetwork(payload));
        });
    }

    #[test]
    fn unbuffered_protocol_fails_the_connection() {
        with_buffered_mux(|running, mux| {
            let payload = NonEmptyBytes::from_slice(&[0x01]).unwrap();
            // Opposite of this wire id is block-fetch responder, which was not buffered.
            running.enqueue_msg(mux, [MuxMessage::FromNetwork(Timestamp(1), PROTO_N2N_BLOCK_FETCH.erase(), payload)]);
            run_until_handler(running).assert_terminated("mux");
        });
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
        let report = t(running.join()).await.unwrap();
        assert_eq!(report.unexpected_exits, vec![mux.name().clone()]);

        trace_guard.defuse();
    }

    fn cbor_byte(byte: u8) -> NonEmptyBytes {
        NonEmptyBytes::from_slice(&[byte]).unwrap()
    }

    async fn block_handler(_state: (), _msg: HandlerMessage, eff: Effects<HandlerMessage>) {
        eff.wait(Duration::from_secs(3600)).await;
    }

    /// The first message waits briefly. Later messages return immediately, which drains a full mailbox.
    async fn hold_then_drain(holding: bool, _msg: HandlerMessage, eff: Effects<HandlerMessage>) -> bool {
        if holding {
            eff.wait(Duration::from_millis(500)).await;
        }
        false
    }

    async fn remember(state: u8, _msg: u8, _eff: Effects<u8>) -> u8 {
        state
    }

    fn ingress_guards() -> amaru_pure_stage::DeserializerGuards {
        super::register_deserializers()
    }

    fn drive(running: &mut SimulationRunning) -> Blocked {
        running.run(Run::default())
    }

    /// Fill a handler that is already inside `wait`, so these messages sit until that wait ends.
    fn fill_handler(running: &mut SimulationRunning, handler: &StageStateRef<HandlerMessage, ()>) {
        for _ in 0..amaru_pure_stage::DEFAULT_MAILBOX_SIZE {
            running.enqueue_msg(handler, [HandlerMessage::Registered(PROTO_TEST.erase())]);
        }
        assert_eq!(running.mailbox_len(handler), amaru_pure_stage::DEFAULT_MAILBOX_SIZE);
    }

    fn proto<'a>(
        running: &'a SimulationRunning,
        mux: &StageStateRef<MuxMessage, State>,
        id: ProtocolId<Erased>,
    ) -> &'a PerProto {
        running.get_state(mux).expect("mux is receiving").muxer.protocols.get(&id).expect("protocol registered")
    }

    struct Handlers {
        mux: StageStateRef<MuxMessage, State>,
        a: StageStateRef<HandlerMessage, ()>,
        b: StageStateRef<HandlerMessage, ()>,
    }

    fn wire_blocked(network: &mut SimulationBuilder) -> Handlers {
        let mux = network.stage("mux", super::stage);
        let mux = network.wire_up(mux, State::new(ConnectionId::initial(), &[], Role::Initiator, test_peer()));
        let a = network.stage("handler-a", block_handler);
        let a = network.wire_up(a, ());
        let b = network.stage("handler-b", block_handler);
        let b = network.wire_up(b, ());
        Handlers { mux, a, b }
    }

    fn start(running: &mut SimulationRunning) {
        // The reader blocks on a network recv. Leaving that child real pins the simulation on
        // Busy and the ingress retry timer never fires. The child is not under test.
        running.use_virtual_child_stages(true);
        running.run(Run::default()).assert_idle();
    }

    #[expect(clippy::wildcard_enum_match_arm)]
    fn counted_retry_arms(running: &SimulationRunning, mux: &Name) {
        let mut armed = false;
        let mut overlaps = 0usize;
        for (_, entry) in running.trace_buffer().lock().iter_entries() {
            match &entry {
                TraceEntry::Suspend(Effect::SetTimeout { at_stage, slot, .. })
                    if at_stage == mux && *slot == INGRESS_RETRY_SLOT =>
                {
                    if armed {
                        overlaps += 1;
                    }
                    armed = true;
                }
                TraceEntry::Suspend(Effect::ClearTimeout { at_stage, slot })
                    if at_stage == mux && *slot == INGRESS_RETRY_SLOT =>
                {
                    armed = false;
                }
                TraceEntry::Input { stage, input }
                    if stage == mux
                        && input.cast_ref::<MuxMessage>().is_ok_and(|msg| matches!(msg, MuxMessage::IngressRetry)) =>
                {
                    armed = false;
                }
                _ => {}
            }
        }
        assert_eq!(overlaps, 0, "a second ingress retry was armed while one was still pending");
    }

    fn register_msg(protocol: ProtocolId<Erased>, handler: StageRef<HandlerMessage>) -> MuxMessage {
        MuxMessage::Register { protocol, frame: Frame::OneCborItem, handler, max_buffer: 64 }
    }

    fn from_network(protocol: ProtocolId<Erased>, byte: u8) -> MuxMessage {
        MuxMessage::FromNetwork(Timestamp(1), protocol.opposite(), cbor_byte(byte))
    }

    /// `assert_trace_contains` drops every resume. The admission result is the following
    /// [`StageResponse::TrySend`](amaru_pure_stage::StageResponse::TrySend) resume, in order.
    fn assert_try_send_resumes(trace: &[TraceEntry], expected: &[TraceMatch<'static>]) {
        let mut found = 0;
        for entry in trace {
            if found < expected.len() && expected[found] == *entry {
                found += 1;
            }
        }
        assert_eq!(found, expected.len(), "try_send responses missing from the trace: {trace:?}");
    }

    fn traced() -> (SimulationBuilder, amaru_pure_stage::trace_buffer::DropGuard) {
        let trace = TraceBuffer::new_shared(200, 1_000_000);
        let guard = TraceBuffer::drop_guard(&trace);
        let network = SimulationBuilder::default().with_trace_buffer(trace);
        (network, guard)
    }

    #[test]
    fn full_handler_keeps_the_frame_and_other_protocols_continue() {
        let _guards = ingress_guards();
        let (mut network, _drop) = traced();
        let handlers = wire_blocked(&mut network);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut running = network.run(rt.handle());
        start(&mut running);

        running.enqueue_msg(&handlers.mux, [register_msg(PROTO_TEST.erase(), handlers.a.as_ref().clone())]);
        drive(&mut running).assert_sleeping();
        fill_handler(&mut running, &handlers.a);

        running.enqueue_msg(&handlers.mux, [register_msg(PROTO_HANDSHAKE.erase(), handlers.b.as_ref().clone())]);
        drive(&mut running).assert_sleeping();
        running.trace_buffer().lock().clear();

        let frame = cbor_byte(0x01);
        running.enqueue_msg(
            &handlers.mux,
            [
                MuxMessage::WantNext(PROTO_TEST.erase()),
                from_network(PROTO_TEST.erase(), 0x01),
                MuxMessage::WantNext(PROTO_HANDSHAKE.erase()),
                from_network(PROTO_HANDSHAKE.erase(), 0x02),
            ],
        );
        drive(&mut running).assert_sleeping();

        let stalled = proto(&running, &handlers.mux, PROTO_TEST.erase());
        assert_eq!(stalled.wanted, 1, "credit stays until the frame is admitted");
        assert_eq!(stalled.incoming.as_ref(), frame.as_ref());
        assert!(stalled.deferred_since.is_some());
        assert!(running.get_state(&handlers.mux).unwrap().muxer.ingress_retry_armed);
        assert_eq!(proto(&running, &handlers.mux, PROTO_HANDSHAKE.erase()).wanted, 0);
        assert!(proto(&running, &handlers.mux, PROTO_HANDSHAKE.erase()).incoming.is_empty());
        counted_retry_arms(&running, handlers.mux.name());

        let mux_name = handlers.mux.name().as_str();
        let trace = running.trace_buffer().lock().hydrate_without_timestamps();
        assert_trace_contains(
            &running,
            &[
                tm_try_send(mux_name, "handler-a", HandlerMessage::FromNetwork(frame)),
                tm_try_send(mux_name, "handler-b", HandlerMessage::FromNetwork(cbor_byte(0x02))),
            ],
        );
        assert_try_send_resumes(
            &trace,
            &[tm_resume_try_send(mux_name, TrySend::Full), tm_resume_try_send(mux_name, TrySend::Queued)],
        );
    }

    #[test]
    fn retry_delivers_once_the_handler_drains() {
        let _guards = ingress_guards();
        let (mut network, _drop) = traced();
        let mux = network.stage("mux", super::stage);
        let mux = network.wire_up(mux, State::new(ConnectionId::initial(), &[], Role::Initiator, test_peer()));
        let handler = network.stage("handler-a", hold_then_drain);
        let handler = network.wire_up(handler, true);

        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut running = network.run(rt.handle());
        start(&mut running);

        running.enqueue_msg(&mux, [register_msg(PROTO_TEST.erase(), handler.as_ref().clone())]);
        let parked = drive(&mut running).assert_sleeping();
        for _ in 0..amaru_pure_stage::DEFAULT_MAILBOX_SIZE {
            running.enqueue_msg(&handler, [HandlerMessage::Registered(PROTO_TEST.erase())]);
        }
        assert_eq!(running.mailbox_len(&handler), amaru_pure_stage::DEFAULT_MAILBOX_SIZE);

        running.enqueue_msg(&mux, [MuxMessage::WantNext(PROTO_TEST.erase()), from_network(PROTO_TEST.erase(), 0x01)]);
        let waiting = drive(&mut running).assert_sleeping();
        assert_eq!(waiting, parked, "the handler drains before the ingress retry");
        assert_eq!(proto(&running, &mux, PROTO_TEST.erase()).wanted, 1);
        assert_eq!(proto(&running, &mux, PROTO_TEST.erase()).incoming.as_ref(), &[0x01]);

        let retry_at = running.run(Run::until(waiting)).assert_sleeping();
        assert_eq!(running.mailbox_len(&handler), 0, "the handler drained before the retry");
        running.trace_buffer().lock().clear();

        running.run(Run::until(retry_at)).assert_idle();
        let delivered = proto(&running, &mux, PROTO_TEST.erase());
        assert_eq!(delivered.wanted, 0);
        assert!(delivered.incoming.is_empty());
        assert!(delivered.deferred_since.is_none());
        assert!(!running.get_state(&mux).unwrap().muxer.ingress_retry_armed);
        let mux_name = mux.name().as_str();
        let trace = running.trace_buffer().lock().hydrate_without_timestamps();
        assert_trace_contains(
            &running,
            &[tm_try_send(mux_name, "handler-a", HandlerMessage::FromNetwork(cbor_byte(0x01)))],
        );
        assert_try_send_resumes(&trace, &[tm_resume_try_send(mux_name, TrySend::Queued)]);
    }

    #[test]
    fn deferred_ingress_faults_the_mux_and_leaves_other_stages() {
        let _guards = ingress_guards();
        let (mut network, _drop) = traced();
        let handlers = wire_blocked(&mut network);
        let bystander = network.stage("bystander", remember);
        let bystander = network.wire_up(bystander, 7u8);

        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut running = network.run(rt.handle());
        start(&mut running);

        running.enqueue_msg(&handlers.mux, [register_msg(PROTO_TEST.erase(), handlers.a.as_ref().clone())]);
        drive(&mut running).assert_sleeping();
        fill_handler(&mut running, &handlers.a);

        running.enqueue_msg(&handlers.mux, [register_msg(PROTO_HANDSHAKE.erase(), handlers.b.as_ref().clone())]);
        drive(&mut running).assert_sleeping();
        for _ in 0..2 {
            running.enqueue_msg(&handlers.b, [HandlerMessage::Registered(PROTO_HANDSHAKE.erase())]);
        }
        let other_mailbox = running.mailbox_len(&handlers.b);

        running.enqueue_msg(
            &handlers.mux,
            [MuxMessage::WantNext(PROTO_TEST.erase()), from_network(PROTO_TEST.erase(), 0x01)],
        );
        let mut wake = drive(&mut running).assert_sleeping();
        assert!(running.get_state(&handlers.mux).is_some(), "still inside the deadline");
        assert!(INGRESS_DEADLINE > crate::protocol::NETWORK_SEND_TIMEOUT, "one retry must land before the deadline");

        // Fault is checked after a retry flush. Wakeups strictly inside the deadline stay up;
        // the wakeup that reaches the deadline closes the connection.
        let still_inside = (INGRESS_DEADLINE.as_secs() / crate::protocol::NETWORK_SEND_TIMEOUT.as_secs()) - 1;
        for _ in 0..still_inside {
            wake = running.run(Run::until(wake)).assert_sleeping();
            assert!(running.get_state(&handlers.mux).is_some(), "retry is still inside the deadline");
            assert_eq!(proto(&running, &handlers.mux, PROTO_TEST.erase()).wanted, 1);
        }

        running.run(Run::until(wake)).assert_terminated(handlers.mux.name());
        assert_eq!(running.get_state(&bystander), Some(&7), "faulting the mux leaves other stages running");
        assert_eq!(running.mailbox_len(&handlers.b), other_mailbox);
    }

    #[test]
    fn registered_is_deferred_when_the_handler_is_full() {
        let _guards = ingress_guards();
        let (mut network, _drop) = traced();
        let mux = network.stage("mux", super::stage);
        let mux = network.wire_up(mux, State::new(ConnectionId::initial(), &[], Role::Initiator, test_peer()));
        let handler = network.stage("handler-a", hold_then_drain);
        let handler = network.wire_up(handler, true);

        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut running = network.run(rt.handle());
        start(&mut running);

        running.enqueue_msg(&handler, [HandlerMessage::FromNetwork(cbor_byte(0x01))]);
        let parked = running.run(Run::default()).assert_sleeping();
        for _ in 0..amaru_pure_stage::DEFAULT_MAILBOX_SIZE {
            running.enqueue_msg(&handler, [HandlerMessage::FromNetwork(cbor_byte(0x01))]);
        }
        assert_eq!(running.mailbox_len(&handler), amaru_pure_stage::DEFAULT_MAILBOX_SIZE);
        running.trace_buffer().lock().clear();

        running.enqueue_msg(&mux, [register_msg(PROTO_TEST.erase(), handler.as_ref().clone())]);
        let waiting = drive(&mut running).assert_sleeping();
        assert_eq!(waiting, parked, "the handler drains before the ingress retry");
        let pending = proto(&running, &mux, PROTO_TEST.erase());
        assert!(pending.registered_pending);
        assert!(pending.deferred_since.is_some());
        assert_eq!(running.mailbox_len(&handler), amaru_pure_stage::DEFAULT_MAILBOX_SIZE);
        let mux_name = mux.name().as_str();
        let trace = running.trace_buffer().lock().hydrate_without_timestamps();
        assert_trace_contains(
            &running,
            &[tm_try_send(mux_name, "handler-a", HandlerMessage::Registered(PROTO_TEST.erase()))],
        );
        assert_try_send_resumes(&trace, &[tm_resume_try_send(mux_name, TrySend::Full)]);

        let retry_at = running.run(Run::until(waiting)).assert_sleeping();
        assert_eq!(running.mailbox_len(&handler), 0);
        running.trace_buffer().lock().clear();
        running.run(Run::until(retry_at)).assert_idle();
        assert!(!proto(&running, &mux, PROTO_TEST.erase()).registered_pending);
        let mux_name = mux.name().as_str();
        let trace = running.trace_buffer().lock().hydrate_without_timestamps();
        assert_trace_contains(
            &running,
            &[tm_try_send(mux_name, "handler-a", HandlerMessage::Registered(PROTO_TEST.erase()))],
        );
        assert_try_send_resumes(&trace, &[tm_resume_try_send(mux_name, TrySend::Queued)]);
    }

    #[test]
    fn ingress_limit_still_applies_while_a_frame_is_deferred() {
        let _guards = ingress_guards();
        let (mut network, _drop) = traced();
        let mux = network.stage("mux", super::stage);
        let mux = network.wire_up(mux, State::new(ConnectionId::initial(), &[], Role::Initiator, test_peer()));
        let handler = network.stage("handler-a", block_handler);
        let handler = network.wire_up(handler, ());
        let bystander = network.stage("bystander", remember);
        let bystander = network.wire_up(bystander, 7u8);

        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut running = network.run(rt.handle());
        start(&mut running);

        running.enqueue_msg(
            &mux,
            [MuxMessage::Register {
                protocol: PROTO_TEST.erase(),
                frame: Frame::OneCborItem,
                handler: handler.as_ref().clone(),
                max_buffer: 1,
            }],
        );
        drive(&mut running).assert_sleeping();
        fill_handler(&mut running, &handler);
        running.enqueue_msg(&mux, [MuxMessage::WantNext(PROTO_TEST.erase()), from_network(PROTO_TEST.erase(), 0x01)]);
        drive(&mut running).assert_sleeping();
        assert_eq!(proto(&running, &mux, PROTO_TEST.erase()).incoming.len(), 1);

        running.enqueue_msg(&mux, [from_network(PROTO_TEST.erase(), 0x02)]);
        drive(&mut running).assert_terminated(mux.name());
        assert_eq!(running.get_state(&bystander), Some(&7));
    }

    #[test]
    fn one_wakeup_retries_deferred_protocols_in_registration_order() {
        let _guards = ingress_guards();
        let (mut network, _drop) = traced();
        let handlers = wire_blocked(&mut network);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut running = network.run(rt.handle());
        start(&mut running);

        // PROTO_TEST's id is greater than handshake. Registration order, not id order, is the retry order.
        running.enqueue_msg(&handlers.mux, [register_msg(PROTO_TEST.erase(), handlers.a.as_ref().clone())]);
        drive(&mut running).assert_sleeping();
        fill_handler(&mut running, &handlers.a);
        running.enqueue_msg(&handlers.mux, [register_msg(PROTO_HANDSHAKE.erase(), handlers.b.as_ref().clone())]);
        drive(&mut running).assert_sleeping();
        fill_handler(&mut running, &handlers.b);
        running.trace_buffer().lock().clear();

        running.enqueue_msg(
            &handlers.mux,
            [
                MuxMessage::WantNext(PROTO_TEST.erase()),
                from_network(PROTO_TEST.erase(), 0x01),
                MuxMessage::WantNext(PROTO_HANDSHAKE.erase()),
                from_network(PROTO_HANDSHAKE.erase(), 0x02),
            ],
        );
        let retry_at = drive(&mut running).assert_sleeping();
        counted_retry_arms(&running, handlers.mux.name());
        let mux_name = handlers.mux.name().clone();
        let sets = running
            .trace_buffer()
            .lock()
            .iter_entries()
            .filter(|(_, entry)| {
                matches!(
                    entry,
                    TraceEntry::Suspend(Effect::SetTimeout { at_stage, slot, .. })
                        if at_stage == &mux_name && *slot == 1
                )
            })
            .count();
        assert_eq!(sets, 1, "two deferred protocols share one wakeup");

        running.trace_buffer().lock().clear();
        running.run(Run::until(retry_at)).assert_sleeping();
        counted_retry_arms(&running, &mux_name);
        let trace = running.trace_buffer().lock().hydrate_without_timestamps();
        assert_trace_contains(
            &running,
            &[
                tm_try_send(mux_name.as_str(), "handler-a", HandlerMessage::FromNetwork(cbor_byte(0x01))),
                tm_try_send(mux_name.as_str(), "handler-b", HandlerMessage::FromNetwork(cbor_byte(0x02))),
            ],
        );
        assert_try_send_resumes(
            &trace,
            &[
                tm_resume_try_send(mux_name.as_str(), TrySend::Full),
                tm_resume_try_send(mux_name.as_str(), TrySend::Full),
            ],
        );
    }

    #[test]
    fn gone_handler_drops_ingress_without_faulting() {
        let _guards = ingress_guards();
        let (mut network, _drop) = traced();
        let mux = network.stage("mux", super::stage);
        let mux = network.wire_up(mux, State::new(ConnectionId::initial(), &[], Role::Initiator, test_peer()));
        let bystander = network.stage("bystander", remember);
        let bystander = network.wire_up(bystander, 7u8);

        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut running = network.run(rt.handle());
        start(&mut running);

        running.enqueue_msg(&mux, [register_msg(PROTO_TEST.erase(), StageRef::named_for_tests("missing"))]);
        drive(&mut running).assert_idle();
        let proto_state = proto(&running, &mux, PROTO_TEST.erase());
        assert!(!proto_state.registered_pending);
        assert!(proto_state.deferred_since.is_none());
        assert!(!running.get_state(&mux).unwrap().muxer.ingress_retry_armed);

        running.enqueue_msg(&mux, [MuxMessage::WantNext(PROTO_TEST.erase()), from_network(PROTO_TEST.erase(), 0x01)]);
        drive(&mut running).assert_idle();
        let proto_state = proto(&running, &mux, PROTO_TEST.erase());
        assert_eq!(proto_state.wanted, 0);
        assert!(proto_state.incoming.is_empty(), "a gone handler cannot accept the frame later");
        assert!(running.get_state(&mux).is_some());
        assert_eq!(running.get_state(&bystander), Some(&7));
        let mux_name = mux.name().as_str();
        let trace = running.trace_buffer().lock().hydrate_without_timestamps();
        assert_trace_contains(
            &running,
            &[
                tm_try_send(mux_name, "missing", HandlerMessage::Registered(PROTO_TEST.erase())),
                tm_try_send(mux_name, "missing", HandlerMessage::FromNetwork(cbor_byte(0x01))),
            ],
        );
        assert_try_send_resumes(
            &trace,
            &[tm_resume_try_send(mux_name, TrySend::Gone), tm_resume_try_send(mux_name, TrySend::Gone)],
        );
    }

    fn payload(byte: u8, len: usize) -> NonEmptyBytes {
        Bytes::from(vec![byte; len]).try_into().unwrap()
    }

    async fn flag(_seen: u8, _msg: Sent, _eff: Effects<Sent>) -> u8 {
        1
    }

    async fn sink(_state: (), _msg: HandlerMessage, _eff: Effects<HandlerMessage>) {}

    /// Real writer and reader. The writer blocks on the network send, so egress can be inspected
    /// while `sending` is true. The loop is capped so a missed breakpoint cannot spin.
    fn with_writer(
        test: impl FnOnce(
            &mut SimulationRunning,
            &StageStateRef<MuxMessage, State>,
            &Name,
            &StageStateRef<Sent, u8>,
            &StageStateRef<Sent, u8>,
            &StageStateRef<Sent, u8>,
        ),
    ) {
        let _guards = ingress_guards();
        let trace_buffer = TraceBuffer::new_shared(200, 1_000_000);
        let drop_guard = TraceBuffer::drop_guard(&trace_buffer);
        let mut network = SimulationBuilder::default().with_trace_buffer(trace_buffer);
        let mux = network.stage("mux", super::stage);
        let conn = ConnectionId::initial();
        let mux = network.wire_up(mux, State::new(conn, &[], Role::Initiator, test_peer()));
        let handler = network.stage("handler", sink);
        let handler = network.wire_up(handler, ());
        let sent_a = network.stage("sent-a", flag);
        let sent_a = network.wire_up(sent_a, 0);
        let sent_b = network.stage("sent-b", flag);
        let sent_b = network.wire_up(sent_b, 0);
        let sent_c = network.stage("sent-c", flag);
        let sent_c = network.wire_up(sent_c, 0);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut running = network.run(rt.handle());
        running.breakpoint("spawn", |eff| matches!(eff, Effect::WireStage { .. }));
        running.enqueue_msg(
            &mux,
            [MuxMessage::Register {
                protocol: PROTO_TEST.erase(),
                frame: Frame::OneCborItem,
                handler: (*handler).clone(),
                max_buffer: 1024,
            }],
        );
        drive_steps(&mut running, 8).assert_breakpoint("spawn");
        let writer = wire_child_name(&running, mux.name(), (conn, (*mux).clone(), Role::Initiator, test_peer()));
        drive_steps(&mut running, 8).assert_breakpoint("spawn");
        let _reader = wire_child_name(&running, mux.name(), (conn, (*mux).clone(), Role::Initiator, test_peer()));
        // Interpret the reader wire-up and stop when the reader blocks on recv.
        let blocked = drive_steps(&mut running, 8);
        assert!(matches!(blocked, Blocked::Busy { .. }), "reader should block on recv, got {blocked:?}");
        test(&mut running, &mux, &writer, &sent_a, &sent_b, &sent_c);
        drop_guard.defuse();
    }

    fn drive_steps(running: &mut SimulationRunning, max: u32) -> Blocked {
        let mut last = Blocked::Idle;
        for _ in 0..max {
            last = running.run(Run::default());
            match &last {
                Blocked::Breakpoint(name) if name.as_str() == "spawn" => return last,
                Blocked::Busy { .. } | Blocked::Idle | Blocked::Sleeping { .. } | Blocked::Terminated(_) => {
                    return last;
                }
                Blocked::Breakpoint(_) => continue,
                Blocked::Deadlock(_) => return last,
            }
        }
        panic!("exceeded {max} steps, last {last:?}");
    }

    fn seen(running: &SimulationRunning, sent: &StageStateRef<Sent, u8>) -> bool {
        *running.get_state(sent).expect("sent stage") == 1
    }

    #[test]
    fn cap_reached_defers_in_order_and_accepts_when_egress_drains() {
        with_writer(|running, mux, _writer, sent_a, sent_b, sent_c| {
            running.enqueue_msg(
                mux,
                [MuxMessage::Send(PROTO_TEST.erase(), payload(1, MAX_SEGMENT_SIZE), StageRef::clone(sent_a))],
            );
            let blocked = drive_steps(running, 16);
            assert!(matches!(blocked, Blocked::Busy { .. }), "{blocked:?}");
            assert!(seen(running, sent_a));
            assert_eq!(proto(running, mux, PROTO_TEST.erase()).outgoing.len(), 0);
            assert!(running.get_state(mux).unwrap().sending);

            running.enqueue_msg(
                mux,
                [MuxMessage::Send(PROTO_TEST.erase(), payload(2, MAX_SEGMENT_SIZE), StageRef::clone(sent_b))],
            );
            let blocked = drive_steps(running, 8);
            assert!(matches!(blocked, Blocked::Busy { .. }), "{blocked:?}");
            assert!(seen(running, sent_b), "room in an empty egress is accepted while the writer is busy");
            assert_eq!(proto(running, mux, PROTO_TEST.erase()).outgoing.len(), MAX_SEGMENT_SIZE);

            running.enqueue_msg(
                mux,
                [
                    MuxMessage::Send(PROTO_TEST.erase(), payload(3, 1), StageRef::clone(sent_c)),
                    MuxMessage::Send(PROTO_TEST.erase(), payload(4, 2), StageRef::clone(sent_b)),
                ],
            );
            // `sent_b` is already true; the second deferred payload shares it only as a reply target.
            // Use the deferred queue itself for order. `sent_c` must stay unanswered.
            let blocked = drive_steps(running, 8);
            assert!(matches!(blocked, Blocked::Busy { .. }), "{blocked:?}");
            let pp = proto(running, mux, PROTO_TEST.erase());
            assert_eq!(pp.outgoing.len(), MAX_SEGMENT_SIZE, "deferred bytes are not appended");
            assert_eq!(pp.deferred.len(), 2);
            assert_eq!(pp.deferred[0].bytes.as_ref(), &[3]);
            assert_eq!(pp.deferred[1].bytes.as_ref(), &[4, 4]);
            assert!(!seen(running, sent_c));

            // The in-flight segment completes. The cap drains, then the two payloads are accepted in order.
            let writer_name = running
                .trace_buffer()
                .lock()
                .iter_entries()
                .find_map(|(_, entry)| match entry {
                    TraceEntry::Suspend(Effect::TrySend { to, msg, .. })
                        if msg.as_ref().type_id() == std::any::TypeId::of::<OutgoingSdu>() =>
                    {
                        Some(to.clone())
                    }
                    TraceEntry::Suspend(_)
                    | TraceEntry::Resume { .. }
                    | TraceEntry::Clock(_)
                    | TraceEntry::Input { .. }
                    | TraceEntry::State { .. }
                    | TraceEntry::Terminated { .. }
                    | TraceEntry::InvalidBytes(..) => None,
                })
                .expect("writer try_send");
            running.complete_external(&writer_name, Ok::<(), crate::network_effects::SendError>(()));
            let blocked = drive_steps(running, 16);
            assert!(matches!(blocked, Blocked::Busy { .. }), "{blocked:?}");
            assert!(seen(running, sent_c));
            let pp = proto(running, mux, PROTO_TEST.erase());
            assert!(pp.deferred.is_empty());
            assert_eq!(pp.outgoing.len(), 3, "each deferred payload is appended once");
            assert_eq!(&pp.outgoing[..], &[3, 4, 4]);
        });
    }

    #[test]
    fn under_cap_is_accepted_in_the_same_transition_while_the_writer_is_busy() {
        with_writer(|running, mux, _writer, sent_a, sent_b, _sent_c| {
            running.enqueue_msg(mux, [MuxMessage::Send(PROTO_TEST.erase(), payload(1, 1), StageRef::clone(sent_a))]);
            assert!(matches!(drive_steps(running, 16), Blocked::Busy { .. }));
            assert!(seen(running, sent_a));
            assert!(running.get_state(mux).unwrap().sending);

            running.enqueue_msg(mux, [MuxMessage::Send(PROTO_TEST.erase(), payload(2, 10), StageRef::clone(sent_b))]);
            assert!(matches!(drive_steps(running, 8), Blocked::Busy { .. }));
            assert!(seen(running, sent_b));
            assert_eq!(proto(running, mux, PROTO_TEST.erase()).outgoing.len(), 10);
            assert!(proto(running, mux, PROTO_TEST.erase()).deferred.is_empty());
        });
    }

    #[test]
    fn message_larger_than_the_buffer_is_split_and_sent_after_its_last_byte() {
        with_writer(|running, mux, writer, sent_a, sent_b, _sent_c| {
            let big = MAX_SEGMENT_SIZE + 64;
            running.enqueue_msg(mux, [MuxMessage::Send(PROTO_TEST.erase(), payload(1, 1), StageRef::clone(sent_a))]);
            assert!(matches!(drive_steps(running, 16), Blocked::Busy { .. }));
            assert!(seen(running, sent_a));
            assert!(running.get_state(mux).unwrap().sending);

            running.enqueue_msg(mux, [MuxMessage::Send(PROTO_TEST.erase(), payload(9, big), StageRef::clone(sent_b))]);
            assert!(matches!(drive_steps(running, 8), Blocked::Busy { .. }));
            assert!(!seen(running, sent_b), "Sent waits for the last byte");
            let pp = proto(running, mux, PROTO_TEST.erase());
            assert_eq!(pp.outgoing.len(), MAX_SEGMENT_SIZE);
            assert!(pp.outgoing.iter().all(|byte| *byte == 9));
            assert_eq!(pp.deferred.len(), 1);
            assert_eq!(pp.deferred[0].bytes.len(), 64);

            running.complete_external(writer, Ok::<(), SendError>(()));
            assert!(matches!(drive_steps(running, 16), Blocked::Busy { .. }));
            assert!(seen(running, sent_b), "Sent follows the last byte into the buffer");
            let pp = proto(running, mux, PROTO_TEST.erase());
            assert!(pp.deferred.is_empty());
            assert_eq!(pp.outgoing.len(), 64);
            assert!(pp.outgoing.len() <= MAX_SEGMENT_SIZE);
        });
    }

    #[test]
    fn egress_buffer_never_exceeds_one_segment() {
        with_writer(|running, mux, _writer, sent_a, sent_b, sent_c| {
            running.enqueue_msg(mux, [MuxMessage::Send(PROTO_TEST.erase(), payload(1, 1), StageRef::clone(sent_a))]);
            assert!(matches!(drive_steps(running, 16), Blocked::Busy { .. }));
            let big = MAX_SEGMENT_SIZE * 3;
            running.enqueue_msg(
                mux,
                [
                    MuxMessage::Send(PROTO_TEST.erase(), payload(2, big), StageRef::clone(sent_b)),
                    MuxMessage::Send(PROTO_TEST.erase(), payload(3, big), StageRef::clone(sent_c)),
                ],
            );
            assert!(matches!(drive_steps(running, 8), Blocked::Busy { .. }));
            let pp = proto(running, mux, PROTO_TEST.erase());
            assert!(pp.outgoing.len() <= MAX_SEGMENT_SIZE);
            assert_eq!(pp.outgoing.len(), MAX_SEGMENT_SIZE);
            assert!(!seen(running, sent_b));
            assert!(!seen(running, sent_c));
            let waiting: usize = pp.deferred.iter().map(|item| item.bytes.len()).sum();
            assert_eq!(waiting, big * 2 - MAX_SEGMENT_SIZE);
        });
    }

    #[test]
    fn writer_send_is_try_send() {
        with_writer(|running, mux, writer, sent_a, _sent_b, _sent_c| {
            running.enqueue_msg(mux, [MuxMessage::Send(PROTO_TEST.erase(), payload(1, 4), StageRef::clone(sent_a))]);
            assert!(matches!(drive_steps(running, 16), Blocked::Busy { .. }));
            let entries: Vec<TraceEntry> =
                running.trace_buffer().lock().iter_entries().map(|(_, entry)| entry).collect();
            let handoff = entries.iter().position(|entry| {
                matches!(
                    entry,
                    TraceEntry::Suspend(Effect::TrySend { from, to, msg })
                        if from == mux.name()
                            && to == writer
                            && msg.as_ref().type_id() == std::any::TypeId::of::<OutgoingSdu>()
                )
            });
            let Some(handoff) = handoff else {
                panic!("writer handoff must be try_send: {entries:?}");
            };
            // The admission result is the resume after the attempt, not a field of the effect.
            let queued = tm_resume_try_send(mux.name().as_str(), TrySend::Queued);
            assert!(
                entries[handoff + 1..].iter().any(|entry| queued == *entry),
                "writer handoff must be queued: {entries:?}"
            );
            assert!(
                entries
                    .iter()
                    .all(|entry| { !matches!(entry, TraceEntry::Suspend(Effect::Send { to, .. }) if to == writer) }),
                "writer handoff must not be a blocking send"
            );
        });
    }

    #[test]
    fn writer_full_keeps_the_segment_until_a_later_try_send() {
        with_writer(|running, mux, writer, sent_a, _sent_b, _sent_c| {
            let writer_ref = StageRef::<OutgoingSdu>::named_for_tests(writer.as_str());
            for _ in 0..amaru_pure_stage::DEFAULT_MAILBOX_SIZE {
                running
                    .enqueue_msg(&writer_ref, [OutgoingSdu { data: payload(0, 1), timeout: Duration::from_secs(1) }]);
            }
            assert!(matches!(drive_steps(running, 16), Blocked::Busy { .. }));
            // The writer took one filler and is blocked in the network send. Top the mailbox up.
            while running.mailbox_len(&writer_ref) < amaru_pure_stage::DEFAULT_MAILBOX_SIZE {
                running
                    .enqueue_msg(&writer_ref, [OutgoingSdu { data: payload(0, 1), timeout: Duration::from_secs(1) }]);
            }
            assert_eq!(running.mailbox_len(&writer_ref), amaru_pure_stage::DEFAULT_MAILBOX_SIZE);

            running.enqueue_msg(mux, [MuxMessage::Send(PROTO_TEST.erase(), payload(7, 5), StageRef::clone(sent_a))]);
            assert!(matches!(drive_steps(running, 16), Blocked::Busy { .. }));
            assert!(seen(running, sent_a), "the cap accepted the bytes before the writer was asked");
            assert!(!running.get_state(mux).unwrap().sending);
            assert_eq!(proto(running, mux, PROTO_TEST.erase()).outgoing.len(), 5);
            assert!(running.get_state(mux).unwrap().muxer.writer_blocked);

            let again = running.now() + crate::protocol::NETWORK_SEND_TIMEOUT;
            let blocked = running.run(Run::until(again));
            assert!(matches!(blocked, Blocked::Busy { .. } | Blocked::Sleeping { .. }), "{blocked:?}");
            assert_eq!(proto(running, mux, PROTO_TEST.erase()).outgoing.len(), 5, "retry must not append again");

            running.complete_external(writer, Ok::<(), crate::network_effects::SendError>(()));
            assert!(matches!(drive_steps(running, 16), Blocked::Busy { .. }));
            assert_eq!(proto(running, mux, PROTO_TEST.erase()).outgoing.len(), 0);
            assert!(running.get_state(mux).unwrap().sending);
        });
    }

    #[test]
    fn writer_gone_faults_the_mux() {
        let _guards = ingress_guards();
        let trace_buffer = TraceBuffer::new_shared(100, 1_000_000);
        let drop_guard = TraceBuffer::drop_guard(&trace_buffer);
        let mut network = SimulationBuilder::default().with_trace_buffer(trace_buffer);
        let mux = network.stage("mux", super::stage);
        let mux = network.wire_up(mux, State::new(ConnectionId::initial(), &[], Role::Initiator, test_peer()));
        let handler = network.stage("handler", sink);
        let handler = network.wire_up(handler, ());
        let sent = network.stage("sent", flag);
        let sent = network.wire_up(sent, 0u8);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut running = network.run(rt.handle());
        running.use_virtual_child_stages(true);
        running.enqueue_msg(
            &mux,
            [
                MuxMessage::Register {
                    protocol: PROTO_TEST.erase(),
                    frame: Frame::OneCborItem,
                    handler: (*handler).clone(),
                    max_buffer: 1024,
                },
                MuxMessage::Send(PROTO_TEST.erase(), payload(1, 4), StageRef::clone(&sent)),
            ],
        );
        let blocked = drive_steps(&mut running, 16);
        assert!(
            matches!(blocked, Blocked::Terminated(ref name) if name.as_str() == mux.name().as_str()),
            "{blocked:?}"
        );
        let mux_name = mux.name().as_str();
        // `assert_trace_contains` drops resumes, so snapshot the admission result first.
        // An earlier Queued (handler registration) must not satisfy the writer outcome.
        let trace = running.trace_buffer().lock().hydrate_without_timestamps();
        assert_trace_contains(&running, &[tm_try_send_type::<OutgoingSdu>(mux_name, "writer")]);
        let handoff = trace.iter().position(|entry| {
            matches!(
                entry,
                TraceEntry::Suspend(Effect::TrySend { from, to, msg })
                    if from.as_str() == mux_name
                        && to.as_str().contains("writer")
                        && msg.as_ref().type_id() == std::any::TypeId::of::<OutgoingSdu>()
            )
        });
        let Some(handoff) = handoff else {
            panic!("writer handoff missing from the trace: {trace:?}");
        };
        let gone = tm_resume_try_send(mux_name, TrySend::Gone);
        assert!(trace[handoff + 1..].iter().any(|entry| gone == *entry), "writer handoff must be gone: {trace:?}");
        drop_guard.defuse();
    }

    #[test]
    fn deferred_sent_is_delivered_in_the_transition_that_frees_room() {
        with_writer(|running, mux, writer, sent_a, sent_b, sent_c| {
            running.enqueue_msg(
                mux,
                [MuxMessage::Send(PROTO_TEST.erase(), payload(1, MAX_SEGMENT_SIZE), StageRef::clone(sent_a))],
            );
            assert!(matches!(drive_steps(running, 16), Blocked::Busy { .. }));
            assert!(seen(running, sent_a));

            running.enqueue_msg(
                mux,
                [MuxMessage::Send(PROTO_TEST.erase(), payload(2, MAX_SEGMENT_SIZE), StageRef::clone(sent_b))],
            );
            assert!(matches!(drive_steps(running, 8), Blocked::Busy { .. }));
            assert!(seen(running, sent_b));

            running.enqueue_msg(mux, [MuxMessage::Send(PROTO_TEST.erase(), payload(3, 1), StageRef::clone(sent_c))]);
            assert!(matches!(drive_steps(running, 8), Blocked::Busy { .. }));
            assert!(!seen(running, sent_c));
            assert_eq!(proto(running, mux, PROTO_TEST.erase()).deferred.len(), 1);
            assert!(!running.get_state(mux).expect("mux").muxer.egress_retry_armed);

            let before = running.now();
            running.complete_external(writer, Ok::<(), SendError>(()));
            assert!(matches!(drive_steps(running, 16), Blocked::Busy { .. }));
            assert!(seen(running, sent_c), "Sent is delivered once the last byte is in the buffer");
            assert_eq!(running.now(), before, "admission must not wait for the egress retry timer");
            assert!(proto(running, mux, PROTO_TEST.erase()).deferred.is_empty());
            assert!(!running.get_state(mux).expect("mux").muxer.egress_retry_armed);
        });
    }

    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    struct Caller {
        mux: StageRef<MuxMessage>,
        admitted: bool,
    }

    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    struct Go {
        proto: ProtocolId<Erased>,
        bytes: NonEmptyBytes,
    }

    async fn caller_step(mut state: Caller, msg: Go, eff: Effects<Go>) -> Caller {
        let timeout = egress_admission_deadline(msg.bytes.len().get());
        let mux = state.mux.clone();
        match eff.call_with_admission(&mux, timeout, move |cr| MuxMessage::Send(msg.proto, msg.bytes, cr)).await {
            CallAdmission::Reply(Sent) => {
                state.admitted = true;
                state
            }
            CallAdmission::NotAdmitted(_) | CallAdmission::TimedOut(_) => eff.terminate().await,
        }
    }

    async fn drop_sent(_state: (), _msg: Sent, _eff: Effects<Sent>) {}

    fn settle(running: &mut SimulationRunning) {
        let blocked = running.run(Run::default());
        assert!(matches!(blocked, Blocked::Sleeping { .. }), "modelled writer should sleep, got {blocked:?}");
    }

    /// Fire wakeups up to `deadline`. `Ok` is the simulated time from `t0` at which the caller was admitted.
    fn await_admission(
        running: &mut SimulationRunning,
        caller: &StageStateRef<Go, Caller>,
        t0: Instant,
        deadline: Instant,
    ) -> Result<Duration, Blocked> {
        for _ in 0..20_000 {
            if running.get_state(caller).is_some_and(|state| state.admitted) {
                return Ok(running.now().saturating_since(t0));
            }
            match running.run(Run::default()) {
                Blocked::Sleeping { next_wakeup } => {
                    // The run above is what delivers `Sent`. Read it before skipping, or the
                    // clock moves on to the next segment and a timely admission looks late.
                    if running.get_state(caller).is_some_and(|state| state.admitted) {
                        return Ok(running.now().saturating_since(t0));
                    }
                    if next_wakeup > deadline {
                        return Err(Blocked::Sleeping { next_wakeup });
                    }
                    assert!(running.skip_to_next_wakeup(Some(next_wakeup)), "wakeup did not fire");
                }
                Blocked::Terminated(name) => return Err(Blocked::Terminated(name)),
                Blocked::Busy { stages, external_effects } => {
                    return Err(Blocked::Busy { stages, external_effects });
                }
                Blocked::Idle => return Err(Blocked::Idle),
                Blocked::Deadlock(blocked) => return Err(Blocked::Deadlock(blocked)),
                Blocked::Breakpoint(_) => {}
            }
        }
        panic!("bandwidth drive exceeded 20000 steps at {:?}", running.now());
    }

    fn with_modelled_writer(
        bps: u64,
        lanes: &[ProtocolId<Erased>],
        body: impl FnOnce(
            &mut SimulationRunning,
            &StageStateRef<MuxMessage, State>,
            &StageStateRef<Go, Caller>,
            &StageStateRef<Sent, ()>,
        ),
    ) {
        let _link = crate::network_effects::modelled_link::install(bps);
        let _guards = super::register_deserializers();
        let mut network = SimulationBuilder::default();
        let mux = network.stage("mux", super::stage);
        let mux_ref = mux.sender();
        let mux = network.wire_up(mux, State::new(ConnectionId::initial(), &[], Role::Initiator, test_peer()));
        let handlers = network.stage("handlers", sink);
        let handlers = network.wire_up(handlers, ());
        let sent = network.stage("sent-sink", drop_sent);
        let sent = network.wire_up(sent, ());
        let caller = network.stage("caller", caller_step);
        let caller = network.wire_up(caller, Caller { mux: mux_ref, admitted: false });
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut running = network.run(rt.handle());
        running
            .override_external_effect::<SendEffect>(usize::MAX, |_| OverrideResult::handled(Ok::<(), SendError>(())));
        running.enqueue_msg(
            &mux,
            lanes.iter().map(|protocol| MuxMessage::Register {
                protocol: *protocol,
                frame: Frame::OneCborItem,
                handler: (*handlers).clone(),
                max_buffer: 1024,
            }),
        );
        settle(&mut running);
        body(&mut running, &mux, &caller, &sent);
    }

    fn send_now(
        running: &mut SimulationRunning,
        mux: &StageStateRef<MuxMessage, State>,
        sent: &StageStateRef<Sent, ()>,
        proto: ProtocolId<Erased>,
        byte: u8,
        len: usize,
    ) {
        running.enqueue_msg(mux, [MuxMessage::Send(proto, payload(byte, len), StageRef::clone(sent))]);
        settle(running);
    }

    /// Reviewer case: a few-byte message behind one 90_112-byte block, writer busy on a small segment.
    #[test]
    fn batch_done_is_admitted_when_the_inflight_segment_drains() {
        let bf = PROTO_N2N_BLOCK_FETCH.erase();
        with_modelled_writer(MIN_PEER_BANDWIDTH_BPS, &[bf], |running, mux, caller, sent| {
            send_now(running, mux, sent, bf, 1, 8);
            assert!(running.get_state(mux).expect("mux").sending);
            send_now(running, mux, sent, bf, 2, 90_112);
            let queued = proto(running, mux, bf);
            assert_eq!(queued.outgoing.len(), MAX_SEGMENT_SIZE);
            assert_eq!(queued.deferred.iter().map(|item| item.bytes.len()).sum::<usize>(), 90_112 - MAX_SEGMENT_SIZE);

            let t0 = running.now();
            running.enqueue_msg(caller, [Go { proto: bf, bytes: payload(3, 4) }]);
            settle(running);
            assert!(!proto(running, mux, bf).deferred.is_empty());
            assert!(!running.get_state(mux).expect("mux").muxer.egress_retry_armed);

            let waited =
                await_admission(running, caller, t0, t0 + egress_admission_deadline(4)).expect("BatchDone admitted");
            // The 8-byte segment's drain. A max-segment wait or the 1 s retry is a miss.
            assert!(waited > Duration::ZERO, "waited {waited:?}");
            assert!(waited < Duration::from_millis(10), "waited {waited:?}");
        });
    }

    /// One short segment is already in the writer. The caller's block is the only
    /// message still to buffer, so its last byte enters when that segment drains.
    fn prime_inflight(
        running: &mut SimulationRunning,
        mux: &StageStateRef<MuxMessage, State>,
        sent: &StageStateRef<Sent, ()>,
        proto_id: ProtocolId<Erased>,
    ) {
        send_now(running, mux, sent, proto_id, 1, 8);
        assert!(running.get_state(mux).expect("mux").sending);
        assert_eq!(proto(running, mux, proto_id).outgoing.len(), 0);
        assert!(proto(running, mux, proto_id).deferred.is_empty());
    }

    #[test]
    fn honest_peer_at_500_kbps_admits_when_nothing_is_queued_ahead() {
        let bf = PROTO_N2N_BLOCK_FETCH.erase();
        let block = crate::blockfetch::BLOCKFETCH_MAX_BLOCK_WIRE_BYTES;
        with_modelled_writer(MIN_PEER_BANDWIDTH_BPS, &[bf], |running, mux, caller, sent| {
            prime_inflight(running, mux, sent, bf);
            let t0 = running.now();
            running.enqueue_msg(caller, [Go { proto: bf, bytes: payload(3, block) }]);
            settle(running);
            // `Sent` has not been delivered: the caller is still inside the call, so its state
            // is not parked on receive. An early `Sent` would leave `admitted` set.
            assert!(
                running.get_state(caller).is_none(),
                "Sent arrived before the last byte fit in the buffer: {:?}",
                running.get_state(caller)
            );
            let pp = proto(running, mux, bf);
            assert_eq!(pp.outgoing.len(), MAX_SEGMENT_SIZE);
            assert!(pp.outgoing.len() <= MAX_SEGMENT_SIZE);
            assert_eq!(pp.deferred.iter().map(|item| item.bytes.len()).sum::<usize>(), block - MAX_SEGMENT_SIZE);
            assert!(!running.get_state(mux).expect("mux").muxer.egress_retry_armed);

            let limit = egress_admission_deadline(block);
            let waited = await_admission(running, caller, t0, t0 + limit).expect("honest peer admitted");
            assert!(waited <= limit, "waited {waited:?} past {limit:?}");
            assert!(waited < Duration::from_millis(10), "waited for more than the in-flight segment: {waited:?}");
            assert!(proto(running, mux, bf).outgoing.len() <= MAX_SEGMENT_SIZE);
        });
    }

    /// One earlier 96 KiB block still fits in this message's own deadline.
    ///
    /// A handler would already have been admitted for that earlier block before
    /// submitting this one, so production does not queue a whole block ahead.
    /// The last byte here enters about two segment-drains later (~2.1 s). The
    /// deadline is one buffer drain plus this message's wire time (~2.623 s).
    #[test]
    fn one_earlier_block_on_the_lane_still_meets_the_own_message_deadline() {
        let bf = PROTO_N2N_BLOCK_FETCH.erase();
        let block = crate::blockfetch::BLOCKFETCH_MAX_BLOCK_WIRE_BYTES;
        with_modelled_writer(MIN_PEER_BANDWIDTH_BPS, &[bf], |running, mux, caller, sent| {
            prime_inflight(running, mux, sent, bf);
            send_now(running, mux, sent, bf, 2, block);
            let queued = proto(running, mux, bf);
            assert_eq!(queued.outgoing.len(), MAX_SEGMENT_SIZE);
            assert!(!queued.deferred.is_empty());

            let t0 = running.now();
            running.enqueue_msg(caller, [Go { proto: bf, bytes: payload(3, block) }]);
            settle(running);
            assert!(
                running.get_state(caller).is_none(),
                "Sent arrived before the earlier block had left the buffer: {:?}",
                running.get_state(caller)
            );
            let limit = egress_admission_deadline(block);
            let waited = await_admission(running, caller, t0, t0 + limit).expect("one queued block still admitted");
            assert!(waited > Duration::from_secs(2), "waited {waited:?}");
            assert!(waited <= limit, "waited {waited:?} past {limit:?}");
            assert!(proto(running, mux, bf).outgoing.len() <= MAX_SEGMENT_SIZE);
        });
    }

    /// Two earlier 96 KiB blocks push the last byte past this message's own deadline.
    ///
    /// A handler submits one message at a time, so two earlier blocks cannot sit
    /// ahead of a live call. If that invariant is broken, the budget is still
    /// not extended for bytes already queued, and the caller is faulted.
    #[test]
    fn two_earlier_blocks_on_the_lane_miss_the_own_message_deadline() {
        let bf = PROTO_N2N_BLOCK_FETCH.erase();
        let block = crate::blockfetch::BLOCKFETCH_MAX_BLOCK_WIRE_BYTES;
        with_modelled_writer(MIN_PEER_BANDWIDTH_BPS, &[bf], |running, mux, caller, sent| {
            prime_inflight(running, mux, sent, bf);
            send_now(running, mux, sent, bf, 2, block);
            send_now(running, mux, sent, bf, 4, block);
            assert!(proto(running, mux, bf).outgoing.len() <= MAX_SEGMENT_SIZE);

            let t0 = running.now();
            running.enqueue_msg(caller, [Go { proto: bf, bytes: payload(3, block) }]);
            settle(running);
            let limit = egress_admission_deadline(block);
            let blocked = await_admission(running, caller, t0, t0 + limit)
                .expect_err("two queued blocks are outside the deadline");
            assert!(matches!(blocked, Blocked::Terminated(ref name) if name == caller.name()), "{blocked:?}");
            let waited = running.now().saturating_since(t0);
            assert!(
                waited > limit.saturating_sub(egress_buffer_drain()),
                "faulted before the message's own wire time: {waited:?}"
            );
            assert!(waited <= limit, "ran past the deadline: {waited:?} > {limit:?}");
            assert!(proto(running, mux, bf).outgoing.len() <= MAX_SEGMENT_SIZE);
        });
    }

    /// Wire time of one full segment, header included, at 500 kbps.
    fn full_segment_wire_time() -> Duration {
        let bytes = u128::try_from(MAX_SEGMENT_SIZE + SEGMENT_HEADER_LEN).unwrap();
        let nanos = bytes * 8 * 1_000_000_000 / u128::from(MIN_PEER_BANDWIDTH_BPS);
        Duration::from_nanos(u64::try_from(nanos).unwrap())
    }

    /// In-flight full segment, buffer already full, then a 65_537-byte message.
    ///
    /// That is the most a sequential handler can have ahead of one call: the
    /// previous message's last byte is already in the buffer, and one segment
    /// may still be on the wire. Returns how long admission took.
    fn await_just_over_one_segment() -> Duration {
        let bf = PROTO_N2N_BLOCK_FETCH.erase();
        let payload_len = MAX_SEGMENT_SIZE + 2;
        assert_eq!(payload_len, 65_537);
        let mut waited = None;
        with_modelled_writer(MIN_PEER_BANDWIDTH_BPS, &[bf], |running, mux, caller, sent| {
            send_now(running, mux, sent, bf, 1, MAX_SEGMENT_SIZE);
            assert!(running.get_state(mux).expect("mux").sending);
            assert_eq!(proto(running, mux, bf).outgoing.len(), 0);
            assert!(proto(running, mux, bf).deferred.is_empty());
            let inflight_at = running.now();

            send_now(running, mux, sent, bf, 2, MAX_SEGMENT_SIZE);
            assert_eq!(running.now(), inflight_at, "filling the buffer must not start another segment");
            assert!(running.get_state(mux).expect("mux").sending);
            assert_eq!(proto(running, mux, bf).outgoing.len(), MAX_SEGMENT_SIZE);
            assert!(proto(running, mux, bf).deferred.is_empty());

            let t0 = running.now();
            running.enqueue_msg(caller, [Go { proto: bf, bytes: payload(3, payload_len) }]);
            settle(running);
            assert!(
                running.get_state(caller).is_none(),
                "Sent arrived before the last two bytes fit: {:?}",
                running.get_state(caller)
            );
            let queued = proto(running, mux, bf);
            assert_eq!(queued.outgoing.len(), MAX_SEGMENT_SIZE);
            assert_eq!(queued.deferred.iter().map(|item| item.bytes.len()).sum::<usize>(), payload_len);

            let limit = egress_admission_deadline(payload_len);
            let admitted = await_admission(running, caller, t0, t0 + limit).expect("65_537-byte message admitted");
            assert!(admitted <= limit, "waited {admitted:?} past {limit:?}");
            assert!(proto(running, mux, bf).outgoing.len() <= MAX_SEGMENT_SIZE);
            waited = Some(admitted);
        });
        waited.expect("admission time")
    }

    /// A 65_537-byte message behind a full in-flight segment and a full buffer
    /// takes two segment drains (~2.097 s). A fixed 1 s slack plus its own wire
    /// time is 2.049 s, so that slack faults an honest peer.
    #[test]
    fn just_over_one_segment_outlasts_a_one_second_slack() {
        let waited = await_just_over_one_segment();
        let two_segments = full_segment_wire_time() + full_segment_wire_time();
        assert_eq!(two_segments, Duration::from_nanos(2_097_376_000));
        assert_eq!(waited, two_segments, "waited {waited:?}");
        // wire millis 1049 + the old 1 s slack.
        assert!(waited > Duration::from_millis(2_049), "waited {waited:?} would have met the old 1 s slack");
    }

    /// The same worst case is inside the buffer-drain deadline, so a peer that
    /// holds 500 kbps is not faulted.
    #[test]
    fn honest_peer_at_500_kbps_admits_just_over_one_segment() {
        let limit = egress_admission_deadline(MAX_SEGMENT_SIZE + 2);
        assert_eq!(limit, Duration::from_nanos(2_097_560_000));
        let waited = await_just_over_one_segment();
        assert!(waited <= limit, "waited {waited:?} past {limit:?}");
        assert!(waited > Duration::from_millis(2_049), "waited {waited:?} did not reach the old miss");
    }
}
