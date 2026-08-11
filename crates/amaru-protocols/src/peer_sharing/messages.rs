// Copyright 2026 PRAGMA
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

//! Peer-sharing wire messages and peer-address CBOR (network-spec §3.11).

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};

use amaru_kernel::cbor;

/// Maximum payload size for peer-sharing frames (network-spec size limit).
pub const MAX_MESSAGE_BYTES: usize = 5760;

/// Wire messages for the peer-sharing mini-protocol.
///
/// CDDL (network-spec §3.11.7):
/// ```text
/// msgShareRequest = [ 0, word8 ]
/// msgSharePeers   = [ 1, peerAddresses ]
/// msgDone         = [ 2 ]
/// ```
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum Message {
    /// Client requests up to `amount` peer addresses.
    ShareRequest { amount: u8 },
    /// Server replies with peer listen addresses (must not exceed the requested amount).
    SharePeers { peers: Vec<SocketAddr> },
    /// Client terminates the protocol.
    Done,
}

impl Message {
    pub fn message_type(&self) -> &'static str {
        match self {
            Message::ShareRequest { .. } => "ShareRequest",
            Message::SharePeers { .. } => "SharePeers",
            Message::Done => "Done",
        }
    }
}

impl cbor::Encode<()> for Message {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        _ctx: &mut (),
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        match self {
            Message::ShareRequest { amount } => {
                e.array(2)?.u16(0)?;
                e.u8(*amount)?;
            }
            Message::SharePeers { peers } => {
                e.array(2)?.u16(1)?;
                encode_peer_list(e, peers)?;
            }
            Message::Done => {
                e.array(1)?.u16(2)?;
            }
        }
        Ok(())
    }
}

impl<'b> cbor::Decode<'b, ()> for Message {
    fn decode(d: &mut cbor::Decoder<'b>, _ctx: &mut ()) -> Result<Self, cbor::decode::Error> {
        let len = d.array()?;
        let label = d.u16()?;
        match label {
            0 => {
                cbor::check_tagged_array_length(0, len, 2)?;
                let amount = d.u8()?;
                Ok(Message::ShareRequest { amount })
            }
            1 => {
                cbor::check_tagged_array_length(1, len, 2)?;
                let peers = decode_peer_list(d)?;
                Ok(Message::SharePeers { peers })
            }
            2 => {
                cbor::check_tagged_array_length(2, len, 1)?;
                Ok(Message::Done)
            }
            _ => Err(cbor::decode::Error::message("unknown peer-sharing message label")),
        }
    }
}

/// Encode peer list matching ouroboros-network `encodeListWith`:
/// empty → definite length 0; non-empty → indefinite-length array.
fn encode_peer_list<W: cbor::encode::Write>(
    e: &mut cbor::Encoder<W>,
    peers: &[SocketAddr],
) -> Result<(), cbor::encode::Error<W::Error>> {
    if peers.is_empty() {
        e.array(0)?;
    } else {
        e.begin_array()?;
        for peer in peers {
            encode_remote_address(e, peer)?;
        }
        e.end()?;
    }
    Ok(())
}

fn decode_peer_list(d: &mut cbor::Decoder<'_>) -> Result<Vec<SocketAddr>, cbor::decode::Error> {
    let len = d.array()?;
    let mut peers = Vec::new();
    match len {
        Some(n) => {
            for _ in 0..n {
                peers.push(decode_remote_address(d)?);
            }
        }
        None => {
            while d.datatype()? != cbor::data::Type::Break {
                peers.push(decode_remote_address(d)?);
            }
            d.skip()?;
        }
    }
    Ok(peers)
}

/// Encode a listen address per network-spec CDDL / `encodeRemoteAddress`.
///
/// ```text
/// peerAddress = [ 0, word32, portNumber ]                                          ; IPv4
///             / [ 1, word32, word32, word32, word32, portNumber ]                  ; IPv6
/// ```
///
/// Word order matches cardano-node / the Haskell `network` package:
/// - IPv4: `HostAddress` (host byte order; LE on supported platforms)
/// - IPv6: `HostAddress6` (four network-order `Word32` chunks)
fn encode_remote_address<W: cbor::encode::Write>(
    e: &mut cbor::Encoder<W>,
    addr: &SocketAddr,
) -> Result<(), cbor::encode::Error<W::Error>> {
    match addr {
        SocketAddr::V4(v4) => {
            e.array(3)?.u16(0)?;
            e.u32(ipv4_host_word(v4.ip()))?;
            e.u16(v4.port())?;
        }
        SocketAddr::V6(v6) => {
            let [w1, w2, w3, w4] = ipv6_host_words(v6.ip());
            e.array(6)?.u16(1)?;
            e.u32(w1)?;
            e.u32(w2)?;
            e.u32(w3)?;
            e.u32(w4)?;
            e.u16(v6.port())?;
        }
    }
    Ok(())
}

fn decode_remote_address(d: &mut cbor::Decoder<'_>) -> Result<SocketAddr, cbor::decode::Error> {
    let len = d.array()?;
    let tag = d.u16()?;
    match tag {
        0 => {
            cbor::check_tagged_array_length(0, len, 3)?;
            let word = d.u32()?;
            let port = d.u16()?;
            Ok(SocketAddr::V4(SocketAddrV4::new(ipv4_from_host_word(word), port)))
        }
        1 => {
            cbor::check_tagged_array_length(1, len, 6)?;
            let w1 = d.u32()?;
            let w2 = d.u32()?;
            let w3 = d.u32()?;
            let w4 = d.u32()?;
            let port = d.u16()?;
            Ok(SocketAddr::V6(SocketAddrV6::new(ipv6_from_host_words([w1, w2, w3, w4]), port, 0, 0)))
        }
        _ => Err(cbor::decode::Error::message("unknown peer address tag")),
    }
}

// NOTE: cardano-node writes the `network` package `HostAddress` as u32, which is in host byte order..
// https://github.com/haskell/network/blob/master/Network/Socket/Types.hsc#L1301
// (supported platforms are all little-endian, which makes LE the standard)
fn ipv4_host_word(ip: &Ipv4Addr) -> u32 {
    u32::from_le_bytes(ip.octets())
}

fn ipv4_from_host_word(word: u32) -> Ipv4Addr {
    Ipv4Addr::from(word.to_le_bytes())
}

// NOTE: cardano-node writes the `network` package `HostAddress6` as four u32s, which are in network byte order.
// https://github.com/haskell/network/blob/master/Network/Socket/Types.hsc#L1332
fn ipv6_host_words(ip: &Ipv6Addr) -> [u32; 4] {
    let o = ip.octets();
    #[expect(clippy::unwrap_used)]
    let o = <[_; 4]>::try_from(o.as_chunks::<4>().0).unwrap();
    o.map(u32::from_be_bytes)
}

fn ipv6_from_host_words(w: [u32; 4]) -> Ipv6Addr {
    let mut o = [0u8; 16];
    o.as_chunks_mut::<4>().0.iter_mut().zip(w.iter()).for_each(|(dst, src)| {
        *dst = src.to_be_bytes();
    });
    Ipv6Addr::from(o)
}

#[cfg(test)]
mod tests {
    use amaru_kernel::{from_cbor_no_leftovers, prop_cbor_roundtrip, to_cbor};
    use proptest::{prelude::*, prop_compose};

    use super::*;

    prop_cbor_roundtrip!(Message, any_message());

    prop_compose! {
        fn any_ipv4()(a in any::<u8>(), b in any::<u8>(), c in any::<u8>(), d in any::<u8>(), port in any::<u16>()) -> SocketAddr {
            SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(a, b, c, d), port))
        }
    }

    prop_compose! {
        fn any_ipv6()(
            o0 in any::<u8>(), o1 in any::<u8>(), o2 in any::<u8>(), o3 in any::<u8>(),
            o4 in any::<u8>(), o5 in any::<u8>(), o6 in any::<u8>(), o7 in any::<u8>(),
            o8 in any::<u8>(), o9 in any::<u8>(), o10 in any::<u8>(), o11 in any::<u8>(),
            o12 in any::<u8>(), o13 in any::<u8>(), o14 in any::<u8>(), o15 in any::<u8>(),
            port in any::<u16>()
        ) -> SocketAddr {
            SocketAddr::V6(SocketAddrV6::new(
                Ipv6Addr::from([o0, o1, o2, o3, o4, o5, o6, o7, o8, o9, o10, o11, o12, o13, o14, o15]),
                port,
                0,
                0,
            ))
        }
    }

    fn any_socket_addr() -> impl Strategy<Value = SocketAddr> {
        prop_oneof![any_ipv4(), any_ipv6()]
    }

    prop_compose! {
        fn any_share_request()(amount in any::<u8>()) -> Message {
            Message::ShareRequest { amount }
        }
    }

    prop_compose! {
        fn any_share_peers()(peers in proptest::collection::vec(any_socket_addr(), 0..8)) -> Message {
            Message::SharePeers { peers }
        }
    }

    fn any_message() -> impl Strategy<Value = Message> {
        prop_oneof![Just(Message::Done), any_share_request(), any_share_peers()]
    }

    #[test]
    fn ipv4_loopback_host_word_matches_le_octets() {
        // 127.0.0.1 as LE host word (Unix Network.Socket HostAddress convention).
        assert_eq!(ipv4_host_word(&Ipv4Addr::new(127, 0, 0, 1)), 0x0100_007f);
        assert_eq!(ipv4_from_host_word(0x0100_007f), Ipv4Addr::new(127, 0, 0, 1));
    }

    #[test]
    fn share_request_cbor_shape() {
        let bytes = to_cbor(&Message::ShareRequest { amount: 10 });
        // [2, 0, 10] as CBOR: 0x82 0x00 0x0a
        assert_eq!(bytes, vec![0x82, 0x00, 0x0a]);
    }

    #[test]
    fn empty_share_peers_uses_definite_empty_array() {
        let bytes = to_cbor(&Message::SharePeers { peers: vec![] });
        // [2, 1, []] : 0x82 0x01 0x80
        assert_eq!(bytes, vec![0x82, 0x01, 0x80]);
    }

    #[test]
    fn done_cbor_shape() {
        let bytes = to_cbor(&Message::Done);
        assert_eq!(bytes, vec![0x81, 0x02]);
    }

    #[test]
    fn definite_length_peer_list_decodes() {
        // Manually build SharePeers with a definite-length inner array of one IPv4.
        let mut bytes = vec![0x82, 0x01]; // outer [2, 1, ...]
        bytes.push(0x81); // definite array of 1
        // [0, word32_le(127.0.0.1)=0x0100007f, port 3001]
        bytes.extend_from_slice(&[0x83, 0x00, 0x1a, 0x01, 0x00, 0x00, 0x7f, 0x19, 0x0b, 0xb9]);
        let msg: Message = from_cbor_no_leftovers(&bytes).unwrap();
        assert_eq!(
            msg,
            Message::SharePeers { peers: vec![SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), 3001))] }
        );
    }
}
