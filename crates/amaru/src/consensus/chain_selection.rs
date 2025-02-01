use pallas_crypto::hash::Hash;

/// Interface to a header for the purpose of chain selection.
pub trait Header {
    /// Hash of the header
    ///
    /// This is used to identify the header in the chain selection.
    /// Header hash is expected to be unique for each header, eg.
    /// $h \neq h' \logeq hhash() \new h'.hash()$.
    fn hash(&self) -> Hash<32>;

    /// Parent hash of the header
    /// Not all headers have a parent, eg. genesis block.
    fn parent(&self) -> Option<Hash<32>>;
}

/// Current state of chain selection process
///
/// Chain selection is parameterised by the header type `H`, in
/// order to better decouple the internals of what's a header from
/// the selection logic
pub struct ChainSelector<H: Header> {
    tip: H,
}

#[derive(Debug, PartialEq)]
pub enum ChainSelection<H: Header> {
    NewTip(H),
    NoChange,
}

impl<H: Header + Clone> ChainSelector<H> {
    pub fn new(tip: H) -> ChainSelector<H> {
        ChainSelector { tip }
    }

    pub fn roll_forward(&mut self, header: H) -> ChainSelection<H> {
        match header.parent() {
            Some(parent) if parent != self.tip.hash() => ChainSelection::NoChange,
            None => panic!("genesis block is not expected to be rolled forward"),
            _ => {
                self.tip = header;
                ChainSelection::NewTip(self.tip.clone())
            }
        }
    }

    pub fn best_chain(&self) -> &H {
        &self.tip
    }
}

#[cfg(test)]
mod tests {

    use super::ChainSelection::*;
    use super::*;

    use pallas_codec::minicbor as cbor;
    use pallas_crypto::hash::{Hash, Hasher};

    #[derive(Debug, PartialEq, Clone, Copy)]
    enum TestHeader {
        TestHeader {
            block_number: u64,
            slot: u64,
            parent: Hash<32>,
        },
        Genesis,
    }

    impl Header for TestHeader {
        fn hash(&self) -> pallas_crypto::hash::Hash<32> {
            self.hash()
        }

        fn parent(&self) -> Option<pallas_crypto::hash::Hash<32>> {
            match self {
                TestHeader::TestHeader { parent, .. } => Some(*parent),
                TestHeader::Genesis => None,
            }
        }
    }

    impl<C> cbor::encode::Encode<C> for TestHeader {
        fn encode<W: cbor::encode::Write>(
            &self,
            e: &mut cbor::Encoder<W>,
            ctx: &mut C,
        ) -> Result<(), cbor::encode::Error<W::Error>> {
            match self {
                TestHeader::TestHeader {
                    block_number,
                    slot,
                    parent,
                } => e
                    .encode(0)?
                    .array(3)?
                    .encode_with(block_number, ctx)?
                    .encode_with(slot, ctx)?
                    .encode_with(parent, ctx)?
                    .ok(),
                TestHeader::Genesis => e.encode(1)?.ok(),
            }
        }
    }

    impl<'b, C> cbor::decode::Decode<'b, C> for TestHeader {
        fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
            let tag = d.u8()?;
            match tag {
                0 => {
                    let block_number = d.decode_with(ctx)?;
                    let slot = d.decode_with(ctx)?;
                    let parent = d.decode_with(ctx)?;
                    Ok(TestHeader::TestHeader {
                        block_number,
                        slot,
                        parent,
                    })
                }
                1 => Ok(TestHeader::Genesis),
                _ => Err(cbor::decode::Error::message(format!("unknown tag {}", tag))),
            }
        }
    }

    impl TestHeader {
        fn hash(&self) -> Hash<32> {
            Hasher::<256>::hash(self.cbor().as_slice())
        }

        fn cbor(&self) -> Vec<u8> {
            let mut buffer = Vec::new();
            cbor::encode(self, &mut buffer)
                .unwrap_or_else(|e| panic!("unable to encode value to CBOR: {e:?}"));
            buffer
        }
    }

    #[test]
    fn extends_the_chain_with_single_header_from_genesis() {
        let mut chain_selector = ChainSelector::new(TestHeader::Genesis);
        let header = TestHeader::TestHeader {
            block_number: 1,
            slot: 0,
            parent: TestHeader::Genesis.hash(),
        };

        let result = chain_selector.roll_forward(header);

        assert_eq!(NewTip(header), result);
    }

    #[test]
    fn do_not_extend_the_chain_given_parent_does_not_match_tip() {
        let mut chain_selector = ChainSelector::new(TestHeader::Genesis);
        let header = TestHeader::TestHeader {
            block_number: 1,
            slot: 0,
            parent: TestHeader::Genesis.hash(),
        };
        let new_header = TestHeader::TestHeader {
            block_number: 1,
            slot: 1,
            parent: TestHeader::Genesis.hash(),
        };

        chain_selector.roll_forward(header);
        let result = chain_selector.roll_forward(new_header);

        assert_eq!(NoChange, result);
    }

    #[test]
    #[should_panic]
    fn panic_when_forward_with_genesis_block() {
        let mut chain_selector = ChainSelector::new(TestHeader::Genesis);

        chain_selector.roll_forward(TestHeader::Genesis);
    }
}
