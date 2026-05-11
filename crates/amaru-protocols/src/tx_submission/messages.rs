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

use std::fmt::Display;

use amaru_kernel::{EraName, NonEmptyBytes, Transaction, TransactionId, cbor, to_cbor};

use crate::tx_submission::Blocking;

/// A transaction id paired with the era it was created in. Encoded on the wire as
/// `[era_index, tx_id]`, matching the Haskell hard-fork combinator's `GenTxId` shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct EraTaggedTxId {
    pub era: EraName,
    pub id: TransactionId,
}

/// A transaction paired with the era it was created in. Encoded on the wire as
/// `[era_index, cbor tx]`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EraTaggedTx {
    pub era: EraName,
    pub tx: Transaction,
}

/// Messages for the txsubmission mini-protocol.
///
/// Each item carries its own era tag via [`EraTaggedTxId`] / [`EraTaggedTx`].
#[derive(Debug, PartialEq, Eq, Clone, serde::Serialize, serde::Deserialize)]
#[repr(u8)]
pub enum Message {
    Init,
    RequestTxIdsBlocking(u16, u16),
    RequestTxIdsNonBlocking(u16, u16),
    RequestTxs(Vec<EraTaggedTxId>),
    ReplyTxIds(Vec<(EraTaggedTxId, u32)>),
    ReplyTxs(Vec<EraTaggedTx>),
    Done,
}

impl Message {
    /// This is copied from the `std::mem` docs, it is the official way.
    fn discriminant(&self) -> u8 {
        // SAFETY: Because `Self` is marked `repr(u8)`, its layout is a `repr(C)` `union`
        // between `repr(C)` structs, each of which has the `u8` discriminant as its first
        // field, so we can read the discriminant without offsetting the pointer.
        unsafe { *<*const _>::from(self).cast::<u8>() }
    }
}

impl PartialOrd for Message {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Message {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.discriminant().cmp(&other.discriminant()).then_with(|| match (self, other) {
            (Message::Init, Message::Init) => std::cmp::Ordering::Equal,
            (Message::RequestTxIdsBlocking(a1, b1), Message::RequestTxIdsBlocking(a2, b2)) => {
                a1.cmp(a2).then_with(|| b1.cmp(b2))
            }
            (Message::RequestTxIdsNonBlocking(a1, b1), Message::RequestTxIdsNonBlocking(a2, b2)) => {
                a1.cmp(a2).then_with(|| b1.cmp(b2))
            }
            (Message::RequestTxs(a1), Message::RequestTxs(a2)) => a1.cmp(a2),
            (Message::ReplyTxIds(a1), Message::ReplyTxIds(a2)) => a1.cmp(a2),
            (Message::ReplyTxs(a1), Message::ReplyTxs(a2)) => {
                let left = to_cbor(a1);
                let right = to_cbor(a2);
                left.cmp(&right)
            }
            (Message::Done, Message::Done) => std::cmp::Ordering::Equal,
            _ => unreachable!(),
        })
    }
}

impl Message {
    pub fn message_type(&self) -> &'static str {
        match self {
            Message::Init => "Init",
            Message::RequestTxIdsBlocking(_, _) => "RequestTxIdsBlocking",
            Message::RequestTxIdsNonBlocking(_, _) => "RequestTxIdsNonBlocking",
            Message::ReplyTxIds(_) => "ReplyTxIds",
            Message::RequestTxs(_) => "RequestTxs",
            Message::ReplyTxs(_) => "ReplyTxs",
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
            Message::RequestTxIdsBlocking(ack, req) => {
                e.array(4)?.u16(0)?;
                e.encode(Blocking::Yes)?;
                e.u16(*ack)?;
                e.u16(*req)?;
            }
            Message::RequestTxIdsNonBlocking(ack, req) => {
                e.array(4)?.u16(0)?;
                e.encode(Blocking::No)?;
                e.u16(*ack)?;
                e.u16(*req)?;
            }
            Message::ReplyTxIds(ids) => {
                e.array(2)?.u16(1)?;
                e.begin_array()?;
                for (id, size) in ids {
                    e.array(2)?;
                    e.encode(id)?;
                    e.u32(*size)?;
                }
                e.end()?;
            }
            Message::RequestTxs(ids) => {
                e.array(2)?.u16(2)?;
                e.begin_array()?;
                for id in ids {
                    e.encode(id)?;
                }
                e.end()?;
            }
            Message::ReplyTxs(txs) => {
                e.array(2)?.u16(3)?;
                e.begin_array()?;
                for tx in txs {
                    e.encode(tx)?;
                }
                e.end()?;
            }
            Message::Done => {
                e.array(1)?.u16(4)?;
            }
            Message::Init => {
                e.array(1)?.u16(6)?;
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
                cbor::check_tagged_array_length(0, len, 4)?;
                let blocking = d.decode()?;
                let ack = d.u16()?;
                let req = d.u16()?;
                match blocking {
                    Blocking::Yes => Ok(Message::RequestTxIdsBlocking(ack, req)),
                    Blocking::No => Ok(Message::RequestTxIdsNonBlocking(ack, req)),
                }
            }
            1 => {
                cbor::check_tagged_array_length(1, len, 2)?;
                Ok(Message::ReplyTxIds(decode_reply_tx_ids(d)?))
            }
            2 => {
                cbor::check_tagged_array_length(2, len, 2)?;
                Ok(Message::RequestTxs(decode_list(d)?))
            }
            3 => {
                cbor::check_tagged_array_length(3, len, 2)?;
                Ok(Message::ReplyTxs(decode_list(d)?))
            }
            4 => {
                cbor::check_tagged_array_length(4, len, 1)?;
                Ok(Message::Done)
            }
            6 => {
                cbor::check_tagged_array_length(6, len, 1)?;
                Ok(Message::Init)
            }
            _ => Err(cbor::decode::Error::message("unknown variant for txsubmission message")),
        }
    }
}

impl cbor::Encode<()> for EraTaggedTxId {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        _ctx: &mut (),
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.array(2)?.u16(self.era.header_variant() as u16)?.encode(self.id)?;
        Ok(())
    }
}

impl<'b> cbor::Decode<'b, ()> for EraTaggedTxId {
    fn decode(d: &mut cbor::Decoder<'b>, _ctx: &mut ()) -> Result<Self, cbor::decode::Error> {
        let era = decode_era_tag(d)?;
        let id = d.decode()?;
        Ok(EraTaggedTxId { era, id })
    }
}

impl cbor::Encode<()> for EraTaggedTx {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        _ctx: &mut (),
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        let bytes = encode_tx(&self.tx).map_err(|_| cbor::encode::Error::message("failed to encode transaction"))?;
        e.array(2)?.u16(self.era.header_variant() as u16)?.tag(cbor::IanaTag::Cbor)?.bytes(&bytes)?;
        Ok(())
    }
}

impl<'b> cbor::Decode<'b, ()> for EraTaggedTx {
    fn decode(d: &mut cbor::Decoder<'b>, _ctx: &mut ()) -> Result<Self, cbor::decode::Error> {
        let era = decode_era_tag(d)?;
        let tag = d.tag()?;
        if tag != cbor::IanaTag::Cbor.tag() {
            return Err(cbor::decode::Error::message(format!(
                "unexpected tag for txsubmission transaction: expected {}, got {}",
                cbor::IanaTag::Cbor.tag(),
                tag
            )));
        }
        let tx = decode_tx(d.bytes()?)?;
        Ok(EraTaggedTx { era, tx })
    }
}

/// Encode the inner transaction body: `[body, witnesses, is_valid, auxiliary_data]`.
fn encode_tx(tx: &Transaction) -> Result<Vec<u8>, cbor::encode::Error<std::convert::Infallible>> {
    let mut bytes = Vec::new();
    let mut e = cbor::Encoder::new(&mut bytes);
    e.array(4)?.encode(&tx.body)?.encode(&tx.witnesses)?.bool(tx.is_expected_valid)?.encode(&tx.auxiliary_data)?;
    Ok(bytes)
}

/// Decode the `[era_index, _]` prefix and return the era. Leaves the decoder positioned at the
/// inner payload of the 2-element array.
///
/// For now we only support the Conway era
fn decode_era_tag(d: &mut cbor::Decoder<'_>) -> Result<EraName, cbor::decode::Error> {
    let len = d.array()?;
    cbor::check_tagged_array_length(0, len, 2)?;
    let variant: u8 = d.u16()?.try_into().map_err(|_| cbor::decode::Error::message("invalid txsubmission era tag"))?;
    let era_name = EraName::from_header_variant(variant).map_err(|e| cbor::decode::Error::message(format!("{e}")))?;
    if era_name != EraName::Conway {
        Err(cbor::decode::Error::message(format!("unsupported era tag {variant}: only Conway is supported")))
    } else {
        Ok(era_name)
    }
}

/// Decode the inner transaction body produced by [`encode_tx`], asserting that
/// no trailing bytes follow the four-element array.
fn decode_tx(bytes: &[u8]) -> Result<Transaction, cbor::decode::Error> {
    let mut d = cbor::Decoder::new(bytes);
    let len = d.array()?;
    cbor::check_tagged_array_length(0, len, 4)?;
    let body = d.decode()?;
    let witnesses = d.decode()?;
    let is_expected_valid = d.bool()?;
    let auxiliary_data = d.decode()?;
    if !d.datatype().is_err_and(|e| e.is_end_of_input()) {
        return Err(cbor::decode::Error::message(format!(
            "leftovers bytes after txsubmission transaction at position {}",
            d.position()
        )));
    }
    Ok(Transaction { body, witnesses, is_expected_valid, auxiliary_data })
}

/// Decode a CBOR array — definite or indefinite-length — of items implementing `Decode`.
fn decode_list<'b, T>(d: &mut cbor::Decoder<'b>) -> Result<Vec<T>, cbor::decode::Error>
where
    T: cbor::Decode<'b, ()>,
{
    let len = d.array()?;
    let mut items = Vec::new();
    match len {
        Some(len) => {
            for _ in 0..len {
                items.push(d.decode()?);
            }
        }
        None => {
            while d.datatype()? != cbor::data::Type::Break {
                items.push(d.decode()?);
            }
            d.skip()?;
        }
    }
    Ok(items)
}

/// Decode the payload of a `ReplyTxIds` message: a (possibly indefinite-length) array of
/// `[[era_index, tx_id], size]` pairs.
fn decode_reply_tx_ids(d: &mut cbor::Decoder<'_>) -> Result<Vec<(EraTaggedTxId, u32)>, cbor::decode::Error> {
    let len = d.array()?;
    let mut items = Vec::new();
    match len {
        Some(len) => {
            for _ in 0..len {
                items.push(decode_reply_tx_id(d)?);
            }
        }
        None => {
            while d.datatype()? != cbor::data::Type::Break {
                items.push(decode_reply_tx_id(d)?);
            }
            d.skip()?;
        }
    }
    Ok(items)
}

/// Decode a single `[[era_index, tx_id], size]` pair from a `ReplyTxIds` payload.
fn decode_reply_tx_id(d: &mut cbor::Decoder<'_>) -> Result<(EraTaggedTxId, u32), cbor::decode::Error> {
    let pair_len = d.array()?;
    cbor::check_tagged_array_length(0, pair_len, 2)?;
    Ok((d.decode()?, d.u32()?))
}

impl Display for Message {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Message::Init => write!(f, "Init"),
            Message::RequestTxIdsBlocking(ack, req) => {
                write!(f, "RequestTxIdsBlocking(ack: {}, req: {})", ack, req,)
            }
            Message::RequestTxIdsNonBlocking(ack, req) => {
                write!(f, "RequestTxIdsNonBlocking(ack: {}, req: {})", ack, req,)
            }
            Message::ReplyTxIds(ids) => {
                write!(
                    f,
                    "ReplyTxIds(ids: [{}])",
                    ids.iter()
                        .map(|(tagged, size)| format!("({}/{}, {})", tagged.era, tagged.id, size))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
            Message::RequestTxs(ids) => {
                write!(
                    f,
                    "RequestTxs(ids: [{}])",
                    ids.iter().map(|tagged| format!("{}/{}", tagged.era, tagged.id)).collect::<Vec<_>>().join(", ")
                )
            }
            Message::ReplyTxs(txs) => {
                write!(
                    f,
                    "ReplyTxs(txs: [{}])",
                    txs.iter()
                        .map(|tagged| format!("{}/{}", tagged.era, tagged.tx.tx_id()))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
            Message::Done => write!(f, "Done"),
        }
    }
}

/// Messages coming directly from the muxer.
#[derive(Debug, PartialEq, Eq, Clone, serde::Serialize, serde::Deserialize)]
pub enum TxSubmissionMessage {
    Registered,
    FromNetwork(NonEmptyBytes),
}

/// Roundtrip property tests for txsubmission messages.
#[cfg(test)]
mod tests {
    use amaru_kernel::{Hash, prop_cbor_roundtrip, to_cbor};
    use prop::collection::vec;
    use proptest::{prelude::*, prop_compose};

    use super::*;
    use crate::tx_submission::tests::create_transaction;

    mod tx_id {
        use super::*;
        prop_cbor_roundtrip!(TransactionId, any_tx_id());
    }
    mod message {
        use super::*;
        prop_cbor_roundtrip!(Message, any_message());
    }
    mod blocking {
        use super::*;
        prop_cbor_roundtrip!(Blocking, any_blocking());
    }

    #[test]
    fn reply_txs_decoding() {
        let tx = create_transaction(0);
        let message = Message::ReplyTxs(vec![EraTaggedTx { era: EraName::Conway, tx }]);
        let encoded = to_cbor(&message);

        let mut d = cbor::Decoder::new(&encoded);
        assert_eq!(d.array().unwrap(), Some(2)); // definite array length
        assert_eq!(d.u16().unwrap(), 3); // reply_txs message tag
        assert_eq!(d.array().unwrap(), None); // indefinite-length array is next

        let era = decode_era_tag(&mut d).unwrap();
        assert_eq!(era, EraName::Conway);
        assert_eq!(d.tag().unwrap(), cbor::IanaTag::Cbor.tag()); // cbor-in-cbor tag
        let tx_bytes = d.bytes().unwrap();

        assert_eq!(tx_bytes[0], 0x84); // definite-length array of size 4
        let mut inner = cbor::Decoder::new(tx_bytes);
        assert_eq!(inner.array().unwrap(), Some(4)); // [body, witnesses, is_valid, aux]
        inner.skip().unwrap(); // body
        inner.skip().unwrap(); // witnesses
        inner.bool().unwrap(); // is_expected_valid
        inner.skip().unwrap(); // auxiliary_data
        assert_eq!(inner.position(), tx_bytes.len()); // inner tx fully consumed

        assert_eq!(d.datatype().unwrap(), cbor::data::Type::Break); // end of indefinite array
        d.skip().unwrap(); // consume the break
        assert_eq!(d.position(), encoded.len()); // outer message fully consumed
    }

    #[test]
    fn reply_tx_ids_decoding() {
        let ids = vec![(EraTaggedTxId { era: EraName::Conway, id: TransactionId::new(Hash::new([1u8; 32])) }, 42)];
        let encoded = to_cbor(&Message::ReplyTxIds(ids));

        let mut d = cbor::Decoder::new(&encoded);
        assert_eq!(d.array().unwrap(), Some(2)); // outer definite array
        assert_eq!(d.u16().unwrap(), 1); // reply_tx_ids message tag
        assert_eq!(d.array().unwrap(), None); // indefinite-length array of items is next

        let expected_era = EraName::Conway;
        assert_eq!(d.array().unwrap(), Some(2)); // per-item [tagged_id, size]
        assert_eq!(decode_era_tag(&mut d).unwrap(), expected_era);
        d.skip().unwrap(); // tx_id bytes
        d.u32().unwrap(); // size

        assert_eq!(d.datatype().unwrap(), cbor::data::Type::Break); // end of indefinite array
        d.skip().unwrap(); // consume the break
        assert_eq!(d.position(), encoded.len()); // outer message fully consumed
    }

    #[test]
    fn request_txs_decoding() {
        let ids = vec![EraTaggedTxId { era: EraName::Conway, id: TransactionId::new(Hash::new([3u8; 32])) }];
        let encoded = to_cbor(&Message::RequestTxs(ids));

        let mut d = cbor::Decoder::new(&encoded);
        assert_eq!(d.array().unwrap(), Some(2)); // outer definite array
        assert_eq!(d.u16().unwrap(), 2); // request_txs message tag
        assert_eq!(d.array().unwrap(), None); // indefinite-length array of ids is next

        let expected_era = EraName::Conway;
        assert_eq!(decode_era_tag(&mut d).unwrap(), expected_era);
        d.skip().unwrap(); // tx_id bytes

        assert_eq!(d.datatype().unwrap(), cbor::data::Type::Break); // end of indefinite array
        d.skip().unwrap(); // consume the break
        assert_eq!(d.position(), encoded.len()); // outer message fully consumed
    }

    #[test]
    fn init_roundtrip() {
        assert_roundtrip(Message::Init);
    }

    #[test]
    fn done_roundtrip() {
        assert_roundtrip(Message::Done);
    }

    #[test]
    fn request_tx_ids_blocking_roundtrip() {
        assert_roundtrip(Message::RequestTxIdsBlocking(3, 7));
    }

    #[test]
    fn request_tx_ids_non_blocking_roundtrip() {
        assert_roundtrip(Message::RequestTxIdsNonBlocking(2, 5));
    }

    #[test]
    fn reply_tx_ids_non_empty_roundtrip() {
        let ids = vec![
            (EraTaggedTxId { era: EraName::Conway, id: TransactionId::new(Hash::new([1u8; 32])) }, 42),
            (EraTaggedTxId { era: EraName::Conway, id: TransactionId::new(Hash::new([2u8; 32])) }, 99),
        ];
        assert_roundtrip(Message::ReplyTxIds(ids));
    }

    #[test]
    fn reply_tx_ids_empty_roundtrip() {
        assert_roundtrip(Message::ReplyTxIds(vec![]));
    }

    #[test]
    fn request_txs_non_empty_roundtrip() {
        let ids = vec![
            EraTaggedTxId { era: EraName::Conway, id: TransactionId::new(Hash::new([3u8; 32])) },
            EraTaggedTxId { era: EraName::Conway, id: TransactionId::new(Hash::new([4u8; 32])) },
        ];
        assert_roundtrip(Message::RequestTxs(ids));
    }

    #[test]
    fn request_txs_empty_roundtrip() {
        assert_roundtrip(Message::RequestTxs(vec![]));
    }

    #[test]
    fn reply_txs_empty_roundtrip() {
        assert_roundtrip(Message::ReplyTxs(vec![]));
    }

    #[test]
    fn reply_tx_ids_mixed_eras_roundtrip() {
        let ids = vec![(EraTaggedTxId { era: EraName::Conway, id: TransactionId::new(Hash::new([1u8; 32])) }, 42)];
        assert_roundtrip(Message::ReplyTxIds(ids));
    }

    // HELPERS

    prop_compose! {
        pub fn any_tx_id()(
            bytes in any::<[u8; 32]>(),
        ) -> TransactionId {
            TransactionId::new(Hash::new(bytes))
        }
    }

    prop_compose! {
        pub fn any_blocking()(
            bool in any::<bool>(),
        ) -> Blocking {
            if bool { Blocking::Yes } else { Blocking::No }
        }
    }

    prop_compose! {
        fn any_ack_req()(ack in 0u16..=1000, req in 0u16..=1000) -> (u16, u16) {
            (ack, req)
        }
    }

    prop_compose! {
        fn any_era_tagged_tx_id()(id in any_tx_id()) -> EraTaggedTxId {
            EraTaggedTxId { era: EraName::Conway, id }
        }
    }

    prop_compose! {
        fn any_tagged_id_and_sizes_vec()(
            ids in vec(any_era_tagged_tx_id(), 0..20),
            sizes in vec(any::<u32>(), 0..20),
        ) -> Vec<(EraTaggedTxId, u32)> {
            ids.into_iter().zip(sizes).collect()
        }
    }

    prop_compose! {
        fn any_tagged_id_vec()(ids in vec(any_era_tagged_tx_id(), 0..20)) -> Vec<EraTaggedTxId> {
            ids
        }
    }

    prop_compose! {
        fn any_tagged_tx_vec()(txs in vec(any_tagged_tx(), 0..10)) -> Vec<EraTaggedTx> {
            txs
        }
    }

    prop_compose! {
        fn any_tagged_tx()(n in 0u64..=1000) -> EraTaggedTx {
            EraTaggedTx { era: EraName::Conway, tx: create_transaction(n) }
        }
    }

    fn init_message() -> impl Strategy<Value = Message> {
        Just(Message::Init)
    }

    prop_compose! {
        fn request_tx_ids_message()((ack, req) in any_ack_req(), blocking in any_blocking()) -> Message {
            match blocking {
                Blocking::Yes => Message::RequestTxIdsBlocking(ack, req),
                Blocking::No => Message::RequestTxIdsNonBlocking(ack, req),
            }
        }
    }

    prop_compose! {
        fn reply_tx_ids_message()(ids in any_tagged_id_and_sizes_vec()) -> Message {
            Message::ReplyTxIds(ids)
        }
    }

    prop_compose! {
        fn request_txs_message()(ids in any_tagged_id_vec()) -> Message {
            Message::RequestTxs(ids)
        }
    }

    prop_compose! {
        fn reply_txs_message()(txs in any_tagged_tx_vec()) -> Message {
            Message::ReplyTxs(txs)
        }
    }

    fn done_message() -> impl Strategy<Value = Message> {
        Just(Message::Done)
    }

    pub fn any_message() -> impl Strategy<Value = Message> {
        prop_oneof![
            1 => init_message(),
            3 => request_tx_ids_message(),
            3 => reply_tx_ids_message(),
            3 => request_txs_message(),
            3 => reply_txs_message(),
            1 => done_message(),
        ]
    }

    fn assert_roundtrip(message: Message) {
        let encoded = to_cbor(&message);
        let decoded: Message = cbor::decode(&encoded).expect("decoding should succeed");
        assert_eq!(message, decoded);
    }
}
