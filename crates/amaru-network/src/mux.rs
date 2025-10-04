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

#![allow(clippy::disallowed_types)]

use crate::protocol::{Erased, ProtocolId};
use acto::{ActoCell, ActoRef, ActoRuntime};
use anyhow::Context;
use binrw::{BinRead, BinWrite};
use bytes::{Buf, Bytes, BytesMut};
use std::{
    collections::{HashMap, VecDeque, hash_map::Entry},
    fmt,
    future::pending,
    io::Cursor,
    pin::Pin,
    sync::LazyLock,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    select,
    time::Instant,
};
use tracing::{Level, instrument};

const MAX_SEGMENT_SIZE: usize = 65535;

/// microseconds part of the wall clock time
#[derive(Debug)]
#[binrw::binrw]
#[brw(big)]
pub struct Timestamp(u32);

impl Timestamp {
    fn now() -> Self {
        static START: LazyLock<Instant> = LazyLock::new(Instant::now);
        Self(START.elapsed().as_micros() as u32)
    }
}

pub struct Sent;

#[derive(Debug)]
pub enum MuxMessage {
    /// Register the given protocol with its ID so that data will be fed into it
    ///
    /// The protocol will get the first invocation for free and must then request
    /// following invocations by sending `WantNext` (for strict flow control).
    Register(ProtocolId<Erased>, Box<dyn Protocol + Send>),
    /// Buffer incoming data for this protocol ID up to the given limit
    /// (this should be followed by Register eventually, to then consume the data)
    ///
    /// Setting the size to zero means that data are dropped without begin buffered
    /// and without tearing down the connection.
    Buffer(ProtocolId<Erased>, usize),
    /// Send the given message on the protocol ID and notify when enqueued in TCP buffer
    Send(ProtocolId<Erased>, Bytes, ActoRef<Sent>),
    /// internal message coming from the TCP stream reader
    FromNetwork(Timestamp, ProtocolId<Erased>, Bytes),
    /// Permit the next invocation of the Protocol with data from the network.
    WantNext(ProtocolId<Erased>),
}

pub trait Protocol: fmt::Debug {
    /// Attempt to consume data from the buffer
    ///
    /// Return Ok(true) if a message was consumed, Ok(false) if not enough data was present,
    /// Err(...) in case of a protocol error (this will tear down the connection).
    fn try_consume(&mut self, buffer: &mut BytesMut) -> anyhow::Result<bool>;
    /// Maximum number of bytes to buffer for this protocol
    ///
    /// The buffer will be preallocated at registration time. If the buffer reaches this
    /// capacity then the connection will be torn down (because the peer didn’t respect
    /// flow control).
    fn max_buffer(&self) -> usize;
}

type WriteTask = Pin<Box<dyn Future<Output = anyhow::Result<Writer>> + Send>>;
type Writer = Pin<Box<dyn tokio::io::AsyncWrite + Send>>;
type Reader = Pin<Box<dyn tokio::io::AsyncRead + Send>>;

pub async fn actor(
    mut cell: ActoCell<MuxMessage, impl ActoRuntime, anyhow::Result<()>>,
    write: Writer,
    read: Reader,
) -> anyhow::Result<()> {
    let mut write = Some(write);

    let mut muxer = Muxer::new();

    let _reader = {
        let me = cell
            .me()
            .contramap(|(ts, proto_id, bytes)| MuxMessage::FromNetwork(ts, proto_id, bytes));
        cell.spawn_supervised("reader", |cell| reader(cell, read, me))
    };

    let mut task: Option<WriteTask> = None;
    let mut pending = Box::pin(pending());

    loop {
        let task2 = task
            .as_mut()
            .map(|t| t.as_mut())
            .unwrap_or_else(|| pending.as_mut());
        let msg = select! {
            res = task2 => {
                write = Some(res.context("writing to socket")?);
                task = muxer.next_segment().map(|(proto_id, bytes)| {
                    #[allow(clippy::expect_used)]
                    let mut write = write.take().expect("write half has just been put back");
                    Box::pin(async move {
                        write_segment(&mut write, proto_id, &bytes).await?;
                        Ok(write)
                    }) as WriteTask
                });
                continue;
            }
            msg = cell.recv() => msg
        };
        let msg = match msg {
            acto::ActoInput::NoMoreSenders => return Ok(()),
            acto::ActoInput::Supervision { result, .. } => {
                return result
                    .map_err(|e| anyhow::Error::msg(e.to_string()))
                    .and_then(|x| x)
                    .context("reader failed");
            }
            acto::ActoInput::Message(msg) => msg,
        };
        match msg {
            MuxMessage::Register(proto_id, proto) => muxer.register(proto_id, proto)?,
            MuxMessage::Buffer(proto_id, limit) => muxer.buffer(proto_id, limit)?,
            MuxMessage::Send(proto_id, bytes, sent) => {
                assert!(!bytes.is_empty());
                muxer.outgoing(proto_id, bytes, sent);
                if let Some(mut write) = write.take()
                    && let Some((proto_id, bytes)) = muxer.next_segment()
                {
                    task = Some(Box::pin(async move {
                        write_segment(&mut write, proto_id, &bytes).await?;
                        Ok(write)
                    }) as WriteTask);
                }
            }
            MuxMessage::FromNetwork(timestamp, proto_id, bytes) => muxer
                .received(timestamp, proto_id, bytes)
                .with_context(|| format!("reading message for protocol {}", proto_id))?,
            MuxMessage::WantNext(proto_id) => muxer
                .want_next(proto_id)
                .with_context(|| format!("reading message for protocol {}", proto_id))?,
        }
    }
}

pub async fn reader(
    mut cell: ActoCell<(), impl ActoRuntime>,
    mut read: Reader,
    target: ActoRef<(Timestamp, ProtocolId<Erased>, Bytes)>,
) -> anyhow::Result<()> {
    let mut buf = BytesMut::with_capacity(65536);

    loop {
        select! {
            // terminate when () is sent or sender ref is dropped
            _ = cell.recv() => return Ok(()),
            msg = read_segment(&mut read, &mut buf) => {
                let (timestamp, proto_id) = msg?;
                target.send((timestamp, proto_id, buf.copy_to_bytes(buf.len())));
            }
        }
    }
}

#[binrw::binrw]
#[brw(big)]
struct Header {
    timestamp: Timestamp,
    proto_id: ProtocolId<Erased>,
    length: u16,
}
const HEADER_LEN: usize = 8;

impl Header {
    pub fn new(proto_id: ProtocolId<Erased>, bytes: &Bytes) -> Self {
        #[allow(clippy::expect_used)]
        Self {
            timestamp: Timestamp::now(),
            proto_id,
            length: bytes
                .len()
                .try_into()
                .expect("trying to send too long segment"),
        }
    }

    #[cfg(test)]
    pub fn to_vec(&self) -> Vec<u8> {
        let mut bytes = Cursor::new(vec![]);
        self.write(&mut bytes).unwrap();
        bytes.into_inner()
    }
}

pub async fn read_segment(
    read: &mut Reader,
    buf: &mut BytesMut,
) -> anyhow::Result<(Timestamp, ProtocolId<Erased>)> {
    let mut header = [0u8; HEADER_LEN];

    read.read_exact(&mut header).await?;
    let Header {
        timestamp,
        proto_id,
        length,
    } = Header::read(&mut Cursor::new(&header))?;

    buf.resize(length.into(), 0);
    read.read_exact(buf.as_mut()).await?;

    Ok((timestamp, proto_id))
}

pub async fn write_segment(
    write: &mut Writer,
    proto_id: ProtocolId<Erased>,
    bytes: &Bytes,
) -> anyhow::Result<()> {
    let mut header = Cursor::new([0u8; HEADER_LEN]);
    Header::new(proto_id, bytes).write(&mut header)?;
    let header = header.into_inner();

    write.write_all(header.as_slice()).await?;
    write.write_all(bytes).await?;

    Ok(())
}

struct Muxer {
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
    pub fn register(
        &mut self,
        proto_id: ProtocolId<Erased>,
        proto: Box<dyn Protocol + Send>,
    ) -> anyhow::Result<()> {
        let pp = self.do_register(proto_id, proto);
        pp.want_next()?;
        Ok(())
    }

    #[instrument(level = Level::DEBUG, skip(self))]
    pub fn buffer(&mut self, proto_id: ProtocolId<Erased>, limit: usize) -> anyhow::Result<()> {
        let pp = self.do_register(proto_id, Box::new(Buffering(limit)));
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
        proto: Box<dyn Protocol + Send>,
    ) -> &mut PerProto {
        if !self.outgoing.contains(&proto_id) {
            self.outgoing.push(proto_id);
        }
        match self.protocols.entry(proto_id) {
            Entry::Occupied(pp) => {
                let pp = pp.into_mut();
                tracing::trace!(want = pp.wanted, "updating registration");
                pp.proto = proto;
                pp
            }
            Entry::Vacant(pp) => pp.insert(PerProto::new(proto)),
        }
    }

    #[instrument(level = Level::DEBUG, skip_all, fields(proto_id))]
    pub fn outgoing(&mut self, proto_id: ProtocolId<Erased>, bytes: Bytes, sent: ActoRef<Sent>) {
        tracing::trace!(proto = %proto_id, bytes = bytes.len(), "enqueueing send");
        #[allow(clippy::expect_used)]
        self.protocols
            .get_mut(&proto_id)
            .ok_or_else(|| anyhow::anyhow!("protocol {} not registered", proto_id))
            .expect("internal error")
            .enqueue_send(bytes, sent);
    }

    #[instrument(level = Level::DEBUG, skip(self))]
    pub fn next_segment(&mut self) -> Option<(ProtocolId<Erased>, Bytes)> {
        tracing::trace!(next = self.next_out, "next segment");
        for idx in (self.next_out..self.outgoing.len()).chain(0..self.next_out) {
            let proto_id = self.outgoing[idx];
            #[allow(clippy::expect_used)]
            let proto = self
                .protocols
                .get_mut(&proto_id)
                .expect("invariant violation");
            let Some(bytes) = proto.next_segment() else {
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
    pub fn received(
        &mut self,
        timestamp: Timestamp,
        proto_id: ProtocolId<Erased>,
        bytes: Bytes,
    ) -> anyhow::Result<()> {
        if let Some(proto) = self.protocols.get_mut(&proto_id) {
            proto.received(timestamp, bytes)
        } else {
            anyhow::bail!("received data for unknown protocol {}", proto_id)
        }
    }

    pub fn want_next(&mut self, proto_id: ProtocolId<Erased>) -> anyhow::Result<()> {
        #[allow(clippy::expect_used)]
        self.protocols
            .get_mut(&proto_id)
            .ok_or_else(|| anyhow::anyhow!("protocol {} not registered", proto_id))
            .expect("internal error")
            .want_next()?;
        Ok(())
    }
}

struct PerProto {
    incoming: BytesMut,
    outgoing: BytesMut,
    sent_bytes: usize,
    notifiers: VecDeque<(ActoRef<Sent>, usize)>,
    wanted: usize,
    proto: Box<dyn Protocol + Send>,
}

impl PerProto {
    pub fn new(proto: Box<dyn Protocol + Send>) -> Self {
        Self {
            incoming: BytesMut::with_capacity(proto.max_buffer()),
            outgoing: BytesMut::with_capacity(proto.max_buffer()),
            sent_bytes: 0,
            notifiers: VecDeque::new(),
            wanted: 0,
            proto,
        }
    }

    pub fn received(&mut self, _timestamp: Timestamp, bytes: Bytes) -> anyhow::Result<()> {
        if self.proto.max_buffer() == 0 {
            tracing::debug!(size = bytes.len(), "ignoring bytes");
            return Ok(());
        }
        tracing::trace!(size = bytes.len(), wanted = self.wanted, "received bytes");
        if self.incoming.len() + bytes.len() > self.proto.max_buffer() {
            tracing::info!(
                buffered = self.incoming.len(),
                msg = bytes.len(),
                "message exceeds buffer"
            );
            anyhow::bail!(
                "message (size {}) plus buffer (size {}) exceeds limit ({})",
                bytes.len(),
                self.incoming.len(),
                self.proto.max_buffer()
            );
        }
        self.incoming.extend(&bytes);
        if self.wanted > 0 && self.proto.try_consume(&mut self.incoming)? {
            tracing::trace!("extracted message");
            self.wanted -= 1;
        }
        Ok(())
    }

    pub fn want_next(&mut self) -> anyhow::Result<()> {
        tracing::trace!(wanted = self.wanted, "wanting next");
        if self.incoming.is_empty() || !self.proto.try_consume(&mut self.incoming)? {
            tracing::trace!("next delivery deferred");
            self.wanted += 1;
        }
        Ok(())
    }

    pub fn enqueue_send(&mut self, bytes: Bytes, sent: ActoRef<Sent>) {
        self.outgoing.extend(&bytes);
        self.notifiers
            .push_back((sent, self.sent_bytes + self.outgoing.len()));
    }

    pub fn next_segment(&mut self) -> Option<Bytes> {
        if self.outgoing.is_empty() {
            return None;
        }
        let size = self.outgoing.len().min(MAX_SEGMENT_SIZE);
        self.sent_bytes += size;
        while let Some((sent, size)) = self.notifiers.front() {
            if self.sent_bytes >= *size {
                sent.send(Sent);
                self.notifiers.pop_front();
            } else {
                break;
            }
        }
        Some(self.outgoing.copy_to_bytes(size))
    }
}

#[derive(Debug)]
struct Buffering(usize);

impl Protocol for Buffering {
    fn try_consume(&mut self, _buffer: &mut BytesMut) -> anyhow::Result<bool> {
        Ok(false)
    }

    fn max_buffer(&self) -> usize {
        self.0
    }
}

#[cfg(test)]
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
            f.debug_tuple("TestProtocol")
                .field(&self.max_buffer())
                .finish()
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

    impl Protocol for TestProtocol {
        fn try_consume(&mut self, buffer: &mut BytesMut) -> anyhow::Result<bool> {
            #[allow(clippy::len_zero)]
            if buffer.len() >= 1 {
                let byte = buffer.get_u8();
                if (1..=10).contains(&byte) {
                    let (bytes, send, _) = &mut *self.0.lock().unwrap();
                    bytes.push(byte);
                    if let Some(tx) = send.take() {
                        tracing::debug!(bytes = bytes.len(), "sending drained bytes");
                        tx.send(mem::take(bytes)).unwrap();
                    } else {
                        tracing::debug!(bytes = bytes.len(), "keeping bytes");
                    }
                    Ok(true)
                } else {
                    anyhow::bail!("Invalid byte value: {}, expected 1-10", byte)
                }
            } else {
                Ok(false)
            }
        }

        fn max_buffer(&self) -> usize {
            self.0.lock().unwrap().2
        }
    }

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
