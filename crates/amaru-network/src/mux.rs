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
    effects::{Network, NetworkOps},
    protocol::{Erased, ProtocolId},
    socket::ConnectionId,
};
use anyhow::Context;
use bytes::{Buf, BufMut, Bytes, BytesMut};
use cbor_data::{Cbor, ErrorKind, ParseError};
use pure_stage::{CallRef, Effects, StageGraph, StageRef, TryInStage};
use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque, hash_map::Entry},
    time::SystemTime,
};
use tracing::{Level, instrument};

const MAX_SEGMENT_SIZE: usize = 65535;

/// microseconds part of the wall clock time
#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Timestamp(u32);

impl Timestamp {
    fn now() -> Self {
        Self(
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
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
            Frame::OneCborItem => match Cbor::checked_prefix(&data) {
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
    Send(ProtocolId<Erased>, Bytes, CallRef<Sent>),
    /// internal message coming from the TCP stream reader
    FromNetwork(Timestamp, ProtocolId<Erased>, Bytes),
    /// Notify that the segment has been written to the TCP stream
    Written,
    /// Permit the next invocation of the Protocol with data from the network.
    WantNext(ProtocolId<Erased>),
}

pub struct State {
    conn: Connection,
    muxer: Muxer,
    sending: bool,
}

enum Connection {
    Unint(ConnectionId),
    Init(StageRef<Bytes>, StageRef<Read>),
}

impl State {
    pub fn new(conn: ConnectionId) -> Self {
        Self {
            conn: Connection::Unint(conn),
            muxer: Muxer::new(),
            sending: false,
        }
    }

    pub fn init(
        &mut self,
        eff: &mut Effects<MuxMessage>,
    ) -> (&mut Muxer, &mut bool, &StageRef<Bytes>, &StageRef<Read>) {
        match &mut self.conn {
            Connection::Unint(conn) => {
                let writer = eff.stage(
                    format!("writer-{}", conn),
                    |(conn, muxer), data, eff| async move {
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
                );
                let writer = eff.wire_up(writer, (*conn, eff.me())).without_state();
                let reader = eff.stage(format!("reader-{}", conn), read_segment);
                let reader = eff.wire_up(reader, (*conn, eff.me())).without_state();
                self.conn = Connection::Init(writer, reader);
            }
            Connection::Init(..) => {}
        }
        let Connection::Init(writer, reader) = &self.conn else {
            unreachable!()
        };
        (&mut self.muxer, &mut self.sending, &writer, &reader)
    }
}

pub async fn stage(mut state: State, msg: MuxMessage, mut eff: Effects<MuxMessage>) -> State {
    let (muxer, sending, writer, reader) = state.init(&mut eff);

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
    writer: &StageRef<Bytes>,
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
            if *sending {
                tracing::trace!(%proto_id, "deferring send");
                muxer.outgoing(proto_id, bytes, sent);
            } else {
                *sending = true;
                eff.send(&writer, Header::encode(proto_id, &bytes)).await;
            }
            Ok(())
        }
        MuxMessage::FromNetwork(timestamp, proto_id, bytes) => {
            muxer
                .received(timestamp, proto_id, bytes, eff)
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
                eff.send(&writer, Header::encode(proto_id, &bytes)).await;
                *sending = true;
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
    pub fn encode(proto_id: ProtocolId<Erased>, bytes: &Bytes) -> Bytes {
        thread_local! {
            static BUFFER: RefCell<BytesMut> = RefCell::new(BytesMut::with_capacity(HEADER_LEN+MAX_SEGMENT_SIZE));
        }
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

pub struct Muxer {
    protocols: HashMap<ProtocolId<Erased>, PerProto>,
    outgoing: Vec<ProtocolId<Erased>>,
    next_out: usize,
}

impl Muxer {
    pub fn new() -> Self {
        Self {
            protocols: HashMap::new(),
            outgoing: Vec::new(),
            next_out: 0,
        }
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
                let (sent, _) = self.notifiers.pop_front().expect("checked above");
                eff.respond(sent, Sent).await;
            } else {
                break;
            }
        }
        Some(self.outgoing.copy_to_bytes(size))
    }
}

#[cfg(notest)]
mod tests {
    use super::*;
    use crate::protocol::{PROTO_HANDSHAKE, PROTO_N2C_CHAIN_SYNC, PROTO_N2N_BLOCK_FETCH};
    use acto::{AcTokio, ActoHandle, ActoInput, ActoRuntime, SupervisionRef, TokioJoinHandle};
    use std::{
        mem,
        sync::{Arc, Mutex},
        time::Duration,
    };
    use tokio::{
        io::{DuplexStream, duplex, split},
        net::{TcpListener, TcpStream},
        runtime::Handle,
        sync::{mpsc, oneshot},
        time::{error::Elapsed, timeout},
    };

    /// Tests with real async behaviour unfortunately need real wall clock sleep time to allow
    /// things to propagate or assert that something doesn’t get propagated. If tests below are
    /// flaky then this value may be too small for the machine running the test.
    const SAFE_SLEEP: Duration = Duration::from_millis(400);

    /// Test protocol that accepts single bytes with values between 1 and 10
    #[derive(Clone)]
    struct TestProtocol(Arc<Mutex<(Vec<u8>, Option<oneshot::Sender<Vec<u8>>>, usize)>>);

    impl fmt::Debug for TestProtocol {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_tuple("TestProtocol").finish()
        }
    }

    impl TestProtocol {
        fn new() -> Self {
            Self(Arc::new(Mutex::new((Vec::new(), None, 1024))))
        }

        fn next(&self) -> oneshot::Receiver<Vec<u8>> {
            let (tx, rx) = oneshot::channel();
            self.0.lock().unwrap().1 = Some(tx);
            rx
        }

        fn drain(&self) -> Vec<u8> {
            mem::take(&mut self.0.lock().unwrap().0)
        }

        fn set_buffer(&self, max_buffer: usize) {
            self.0.lock().unwrap().2 = max_buffer;
        }
    }

    // impl Protocol for TestProtocol {
    //     fn try_consume(&mut self, buffer: &mut BytesMut) -> anyhow::Result<bool> {
    //         #[allow(clippy::len_zero)]
    //         if buffer.len() >= 1 {
    //             let byte = buffer.get_u8();
    //             if (1..=10).contains(&byte) {
    //                 let (bytes, send, _) = &mut *self.0.lock().unwrap();
    //                 bytes.push(byte);
    //                 if let Some(tx) = send.take() {
    //                     tracing::debug!(bytes = bytes.len(), "sending drained bytes");
    //                     tx.send(mem::take(bytes)).unwrap();
    //                 } else {
    //                     tracing::debug!(bytes = bytes.len(), "keeping bytes");
    //                 }
    //                 Ok(true)
    //             } else {
    //                 anyhow::bail!("Invalid byte value: {}, expected 1-10", byte)
    //             }
    //         } else {
    //             Ok(false)
    //         }
    //     }

    //     fn max_buffer(&self) -> usize {
    //         self.0.lock().unwrap().2
    //     }
    // }

    async fn setup_tcp() -> (
        AcTokio,
        Box<TestProtocol>,
        TcpStream,
        SupervisionRef<MuxMessage, TokioJoinHandle<anyhow::Result<()>>>,
    ) {
        tracing_subscriber::fmt()
            .with_test_writer()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .try_init()
            .ok();

        // Create a TCP listener on an ephemeral port
        tracing::info!("binding to 127.0.0.1:0");
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let server_addr = listener.local_addr().unwrap();

        // Spawn server task
        let runtime = AcTokio::from_handle("test_server", Handle::current());
        let test_proto = Box::new(TestProtocol::new());
        let server_task = tokio::spawn(async move { listener.accept().await.unwrap().0 });

        // Create client connection
        let client_stream = TcpStream::connect(server_addr).await.unwrap();
        let (reader, writer) = server_task.await.unwrap().into_split();

        let cell = runtime.spawn_actor("server", |cell| {
            actor(cell, Box::pin(writer), Box::pin(reader))
        });

        // Register test protocol for protocol ID 5 (PROTO_N2C_CHAIN_SYNC)
        let proto_id = PROTO_N2C_CHAIN_SYNC.erase();
        cell.me
            .send(MuxMessage::Register(proto_id, test_proto.clone()));

        (runtime, test_proto, client_stream, cell)
    }

    fn setup_duplex(
        buffer: usize,
    ) -> (
        AcTokio,
        Box<TestProtocol>,
        DuplexStream,
        SupervisionRef<MuxMessage, TokioJoinHandle<anyhow::Result<()>>>,
    ) {
        tracing_subscriber::fmt()
            .with_test_writer()
            .with_max_level(Level::TRACE)
            .try_init()
            .ok();

        let runtime = AcTokio::from_handle("test_server", Handle::current());
        let test_proto = Box::new(TestProtocol::new());
        let (tester, muxer) = duplex(buffer);
        let (reader, writer) = split(muxer);

        let cell = runtime.spawn_actor("server", |cell| {
            actor(cell, Box::pin(writer), Box::pin(reader))
        });

        // Register test protocol for protocol ID 5 (PROTO_N2C_CHAIN_SYNC)
        let proto_id = PROTO_N2C_CHAIN_SYNC.erase();
        cell.me
            .send(MuxMessage::Register(proto_id, test_proto.clone()));

        (runtime, test_proto, tester, cell)
    }

    async fn t<F, T>(f: F) -> anyhow::Result<T>
    where
        F: Future<Output = T>,
    {
        Ok(timeout(SAFE_SLEEP, f).await?)
    }

    async fn te<F, T, E>(f: F) -> anyhow::Result<T>
    where
        F: Future<Output = Result<T, E>>,
        E: std::error::Error + Send + Sync + 'static,
    {
        Ok(timeout(SAFE_SLEEP, f).await??)
    }

    async fn to<F, T>(f: F) -> anyhow::Result<T>
    where
        F: Future<Output = Option<T>>,
    {
        timeout(SAFE_SLEEP, f)
            .await?
            .ok_or_else(|| anyhow::anyhow!("empty Option"))
    }

    #[track_caller]
    fn assert_elapsed<T: std::fmt::Debug>(err: anyhow::Result<T>) {
        err.unwrap_err().downcast::<Elapsed>().unwrap();
    }

    async fn send(proto_id: ProtocolId<Erased>, payload: &[u8], client_stream: &mut TcpStream) {
        let payload = Bytes::copy_from_slice(payload);
        let header = Header::new(proto_id, &payload).to_vec();
        client_stream.write_all(&header).await.unwrap();
        client_stream.write_all(&payload).await.unwrap();
    }

    #[tokio::test]
    async fn test_tcp_connection_with_protocol_5() {
        let (_runtime, test_proto, mut client_stream, cell) = setup_tcp().await;

        let recv = test_proto.next();
        send(PROTO_N2C_CHAIN_SYNC.erase(), &[1], &mut client_stream).await;
        assert_eq!(recv.await.unwrap(), vec![1u8]);

        // test flow control: block until want next
        let mut recv = test_proto.next();
        send(PROTO_N2C_CHAIN_SYNC.erase(), &[2], &mut client_stream).await;
        timeout(SAFE_SLEEP, &mut recv).await.unwrap_err();

        cell.me
            .send(MuxMessage::WantNext(PROTO_N2C_CHAIN_SYNC.erase()));
        assert_eq!(recv.await.unwrap(), vec![2u8]);

        // test flow control: send immediately if next already wanted
        cell.me
            .send(MuxMessage::WantNext(PROTO_N2C_CHAIN_SYNC.erase()));
        let recv = test_proto.next();
        send(PROTO_N2C_CHAIN_SYNC.erase(), &[3], &mut client_stream).await;
        assert_eq!(recv.await.unwrap(), vec![3u8]);
    }

    #[tokio::test]
    async fn test_tcp_wrong_message() {
        let (_runtime, test_proto, mut client_stream, cell) = setup_tcp().await;

        send(PROTO_N2C_CHAIN_SYNC.erase(), &[11], &mut client_stream).await;
        let err = cell.handle.join().await.unwrap().unwrap_err();
        let msg = format!("{:#}", err);
        assert!(
            msg.contains("protocol 5"),
            "error didn't contain \"protocol 5\": {}",
            msg
        );
        assert!(
            msg.contains("Invalid byte") && msg.contains("11"),
            "error didn't contain \"Invalid byte ... 11\": {}",
            msg
        );
        assert_eq!(test_proto.drain(), vec![0u8; 0]);
    }

    #[tokio::test]
    async fn test_tcp_wrong_protocol() {
        let (_runtime, test_proto, mut client_stream, cell) = setup_tcp().await;

        send(PROTO_N2N_BLOCK_FETCH.erase(), &[1], &mut client_stream).await;
        let err = cell.handle.join().await.unwrap().unwrap_err();
        let msg = format!("{:#}", err);
        assert!(
            msg.contains("protocol 3"),
            "error didn't contain \"protocol 3\": {}",
            msg
        );
        assert_eq!(test_proto.drain(), vec![0u8; 0]);
    }

    #[tokio::test]
    async fn test_tcp_buffer_protocol() {
        let (_runtime, test_proto, mut client_stream, cell) = setup_tcp().await;
        test_proto.set_buffer(1);

        cell.me
            .send(MuxMessage::Buffer(PROTO_N2N_BLOCK_FETCH.erase(), 1));
        let mut recv = test_proto.next();
        send(PROTO_N2N_BLOCK_FETCH.erase(), &[1], &mut client_stream).await;
        timeout(SAFE_SLEEP, &mut recv).await.unwrap_err();

        cell.me.send(MuxMessage::Register(
            PROTO_N2N_BLOCK_FETCH.erase(),
            test_proto.clone(),
        ));
        assert_eq!(recv.await.unwrap(), vec![1u8]);

        // test discarding of messages (which we need after demoting a hot peer to warm)
        let mut recv = test_proto.next();
        send(PROTO_N2N_BLOCK_FETCH.erase(), &[1], &mut client_stream).await;
        timeout(SAFE_SLEEP, &mut recv).await.unwrap_err();
        // this discards the message buffered above
        cell.me
            .send(MuxMessage::Buffer(PROTO_N2N_BLOCK_FETCH.erase(), 0));
        send(PROTO_N2N_BLOCK_FETCH.erase(), &[1, 2], &mut client_stream).await;
        timeout(SAFE_SLEEP, &mut recv).await.unwrap_err();

        cell.me.send(MuxMessage::Register(
            PROTO_N2N_BLOCK_FETCH.erase(),
            test_proto.clone(),
        ));
        // consume the implied WantNext
        send(PROTO_N2N_BLOCK_FETCH.erase(), &[4], &mut client_stream).await;
        assert_eq!(recv.await.unwrap(), vec![4u8]);

        let mut recv = test_proto.next();
        send(PROTO_N2N_BLOCK_FETCH.erase(), &[5], &mut client_stream).await;
        // filling the buffer should work
        timeout(SAFE_SLEEP, &mut recv).await.unwrap_err();

        // exceeding the buffer should not
        send(PROTO_N2N_BLOCK_FETCH.erase(), &[6, 7], &mut client_stream).await;
        let err = cell.handle.join().await.unwrap().unwrap_err();
        let msg = format!("{:#}", err);
        assert!(
            msg.contains("protocol 3")
                && msg.contains("size 1")
                && msg.contains("size 2")
                && msg.contains("limit (1)"),
            "error didn't contain \"protocol 3...size 2...size 1...limit (1)\": {}",
            msg
        );
        // the TestProtocol hasn’t received any of the data
        assert_eq!(test_proto.drain(), vec![0u8; 0]);
    }

    #[tokio::test]
    async fn test_tcp_send() {
        let (runtime, _test_proto, mut client_stream, cell) = setup_tcp().await;

        let (tx, mut rx) = mpsc::channel(10);
        let tester = runtime
            .spawn_actor("tester", async move |mut cell: ActoCell<Sent, _, ()>| {
                let mut count = 0usize;
                while let ActoInput::Message(_) = cell.recv().await {
                    count += 1;
                    tx.send(count).await.unwrap();
                }
            })
            .me;

        let bytes = &[1, 2, 3][..];
        cell.me.send(MuxMessage::Send(
            PROTO_N2C_CHAIN_SYNC.erase(),
            Bytes::copy_from_slice(bytes),
            tester,
        ));

        assert_eq!(rx.recv().await.unwrap(), 1);
        let mut buffer = vec![0; 11];
        client_stream
            .read_exact(buffer.as_mut_slice())
            .await
            .unwrap();

        let mut buffer = Cursor::new(buffer);
        let header = Header::read(&mut buffer).unwrap();
        assert_eq!(header.proto_id, PROTO_N2C_CHAIN_SYNC.erase());
        assert_eq!(usize::from(header.length), bytes.len());
        assert_eq!(buffer.position(), HEADER_LEN as u64);
        let buffer = buffer.into_inner();
        assert_eq!(&buffer[HEADER_LEN..], bytes);
    }

    #[tokio::test]
    async fn test_muxing() {
        let (runtime, _test_proto, mut client, cell) = setup_duplex(1000);
        let (tx, mut sent) = mpsc::channel(10);
        let tester = runtime
            .spawn_actor("tester", async move |mut cell: ActoCell<u8, _, ()>| {
                while let ActoInput::Message(msg) = cell.recv().await {
                    tx.send(msg).await.unwrap();
                }
            })
            .me;

        // sequence of registration is the sequence of round-robin
        cell.me
            .send(MuxMessage::Buffer(PROTO_N2N_BLOCK_FETCH.erase(), 0));
        cell.me.send(MuxMessage::Buffer(PROTO_HANDSHAKE.erase(), 0));

        let send = |msg: u8, len: usize, proto_id: ProtocolId<Erased>| {
            let bytes = vec![msg; len];
            cell.me.send(MuxMessage::Send(
                proto_id,
                Bytes::copy_from_slice(&bytes),
                tester.contramap(move |_| msg),
            ));
        };

        // fill buffer and plus 32 bytes in the write task
        send(1, 1024, PROTO_N2C_CHAIN_SYNC.erase());
        assert_eq!(to(sent.recv()).await.unwrap(), 1);

        // put 1024 bytes into the proto buffer
        send(2, 1024, PROTO_N2C_CHAIN_SYNC.erase());
        assert_elapsed(t(sent.recv()).await);
        // put 10 bytes into the proto buffer
        send(3, 10, PROTO_N2C_CHAIN_SYNC.erase());

        // fill segments for other two protocols
        send(4, 66000, PROTO_HANDSHAKE.erase());
        send(5, 66000, PROTO_N2N_BLOCK_FETCH.erase());
        assert_elapsed(t(sent.recv()).await);

        let mut recv = async |msg: &[(u8, usize)], proto_id: ProtocolId<Erased>| {
            let len = msg.iter().map(|(_, len)| *len).sum::<usize>();
            tracing::info!("recv: {} bytes for protocol {}", len, proto_id);
            let mut buf = BytesMut::zeroed(len + HEADER_LEN);
            te(client.read_exact(buf.as_mut())).await.unwrap();
            let mut buf = Cursor::new(buf);
            let header = Header::read(&mut buf).unwrap();
            assert_eq!(header.proto_id, proto_id);
            assert_eq!(usize::from(header.length), len);
            assert_eq!(buf.position(), HEADER_LEN as u64);
            let mut buf = buf.into_inner();
            buf = buf.split_off(HEADER_LEN);
            for (msg, len) in msg {
                let rest = buf.split_off(*len);
                assert!(
                    buf.iter().all(|b| b == msg),
                    "expected {} bytes of {}, got {:?}",
                    len,
                    msg,
                    buf
                );
                buf = rest;
            }
        };

        recv(&[(1, 1024)], PROTO_N2C_CHAIN_SYNC.erase()).await;
        recv(&[(5, 65535)], PROTO_N2N_BLOCK_FETCH.erase()).await;
        recv(&[(4, 65535)], PROTO_HANDSHAKE.erase()).await;
        assert_eq!(to(sent.recv()).await.unwrap(), 2);
        assert_eq!(to(sent.recv()).await.unwrap(), 3);
        recv(&[(2, 1024), (3, 10)], PROTO_N2C_CHAIN_SYNC.erase()).await;
        assert_eq!(to(sent.recv()).await.unwrap(), 5);
        recv(&[(5, 465)], PROTO_N2N_BLOCK_FETCH.erase()).await;
        assert_eq!(to(sent.recv()).await.unwrap(), 4);
        recv(&[(4, 465)], PROTO_HANDSHAKE.erase()).await;
    }
}
