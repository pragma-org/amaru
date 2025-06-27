use pallas_addresses::Address;
use pallas_codec::{minicbor as cbor, minicbor::data::IanaTag, utils::Bytes};
use pallas_crypto::hash::{Hash, Hasher};
use pallas_primitives::{
    alonzo,
    conway::{
        KeepRaw, MintedDatumOption, MintedTransactionOutput, NativeScript, PseudoScript, Value,
    },
    DatumHash, NonEmptyKeyValuePairs, PlutusData,
};

// --------------------------------------------------- MemoizedTransactionOutput

#[derive(Debug, Clone)]
pub struct MemoizedTransactionOutput {
    is_legacy: bool,
    pub address: Address,
    pub value: Value,
    pub datum: MemoizedDatum,
    pub script: Option<MemoizedScript>,
}

impl<'b, C> cbor::Decode<'b, C> for MemoizedTransactionOutput {
    // TODO: This implementation still copies the bytes when we convert from the minted version to
    // the owned version. To avoid that, we should actually rewrite the decoders for all the
    // memoized structures, so that they can retain their original bytes.
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let minted: MintedTransactionOutput<'_> = d.decode_with(ctx)?;
        Self::try_from(minted).map_err(cbor::decode::Error::message)
    }
}

impl<C> cbor::Encode<C> for MemoizedTransactionOutput {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        if self.is_legacy {
            e.begin_array()?;
            e.bytes(&self.address.to_vec())?;
            e.encode_with(&self.value, ctx)?;
            match self.datum {
                MemoizedDatum::None => (),
                MemoizedDatum::Hash(hash) => {
                    e.bytes(&hash[..])?;
                }
                MemoizedDatum::Inline(..) => unreachable!("legacy output with inline datum ?!"),
            }
            e.end()?;
        } else {
            e.begin_map()?;

            e.u8(0)?;
            e.bytes(&self.address.to_vec())?;

            e.u8(1)?;
            e.encode_with(&self.value, ctx)?;

            match &self.datum {
                MemoizedDatum::None => (),
                MemoizedDatum::Hash(hash) => {
                    e.u8(2)?;
                    e.array(2)?;
                    e.u8(0)?;
                    e.bytes(&hash[..])?;
                }
                MemoizedDatum::Inline(data) => {
                    e.u8(2)?;
                    e.array(2)?;
                    e.u8(1)?;
                    e.tag(IanaTag::Cbor)?;
                    e.bytes(data.original_bytes())?;
                }
            }

            match &self.script {
                None => (),
                Some(script) => {
                    e.u8(3)?;
                    e.tag(IanaTag::Cbor)?;
                    e.bytes(script_original_bytes(script))?;
                }
            }

            e.end()?;
        }

        Ok(())
    }
}

impl<'a> TryFrom<MintedTransactionOutput<'a>> for MemoizedTransactionOutput {
    type Error = String;

    fn try_from(
        output: MintedTransactionOutput<'a>,
    ) -> Result<MemoizedTransactionOutput, Self::Error> {
        match output {
            MintedTransactionOutput::Legacy(output) => Ok(MemoizedTransactionOutput {
                is_legacy: true,
                address: Address::from_bytes(&output.address)
                    .map_err(|e| format!("invalid address: {e:?}"))?,
                value: from_legacy_value(output.amount)?,
                datum: MemoizedDatum::from(output.datum_hash),
                script: None,
            }),
            MintedTransactionOutput::PostAlonzo(output) => Ok(MemoizedTransactionOutput {
                is_legacy: false,
                address: Address::from_bytes(&output.address)
                    .map_err(|e| format!("invalid address: {e:?}"))?,
                value: output.value,
                datum: MemoizedDatum::from(output.datum_option),
                script: output.script_ref.map(|script| match script.unwrap() {
                    PseudoScript::NativeScript(native_script) => {
                        PseudoScript::NativeScript(MemoizedNativeScript::from(native_script))
                    }
                    PseudoScript::PlutusV1Script(plutus_script) => {
                        PseudoScript::PlutusV1Script(plutus_script)
                    }
                    PseudoScript::PlutusV2Script(plutus_script) => {
                        PseudoScript::PlutusV2Script(plutus_script)
                    }
                    PseudoScript::PlutusV3Script(plutus_script) => {
                        PseudoScript::PlutusV3Script(plutus_script)
                    }
                }),
            }),
        }
    }
}

fn from_legacy_value(value: alonzo::Value) -> Result<Value, String> {
    match value {
        alonzo::Value::Coin(coin) => Ok(Value::Coin(coin)),
        alonzo::Value::Multiasset(coin, assets) if assets.is_empty() => Ok(Value::Coin(coin)),
        alonzo::Value::Multiasset(coin, assets) => Ok(Value::Multiasset(
            coin,
            NonEmptyKeyValuePairs::try_from(
                assets
                    .to_vec()
                    .into_iter()
                    .map(|(policy_id, tokens)| {
                        Ok((
                            policy_id,
                            NonEmptyKeyValuePairs::try_from(
                                tokens
                                    .to_vec()
                                    .into_iter()
                                    .map(|(asset_name, quantity)| {
                                        Ok((
                                            asset_name,
                                            quantity.try_into().map_err(|e| {
                                                format!("invalid quantity in legacy output: {e}")
                                            })?,
                                        ))
                                    })
                                    .collect::<Result<Vec<_>, String>>()?,
                            )
                            .map_err(|_| "empty tokens under a policy?")?,
                        ))
                    })
                    .collect::<Result<Vec<_>, String>>()?,
            )
            .map_err(|_| "assets cannot be empty due to pattern-guard")?,
        )),
    }
}

// -------------------------------------------------------------- MemoizedScript

pub type MemoizedScript = PseudoScript<MemoizedNativeScript>;

pub fn script_original_bytes(script: &MemoizedScript) -> &[u8] {
    match script {
        MemoizedScript::NativeScript(native) => native.original_bytes(),
        MemoizedScript::PlutusV1Script(plutus) => plutus.as_ref(),
        MemoizedScript::PlutusV2Script(plutus) => plutus.as_ref(),
        MemoizedScript::PlutusV3Script(plutus) => plutus.as_ref(),
    }
}

// --------------------------------------------------------------- MemoizedDatum

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub enum MemoizedDatum {
    None,
    Hash(DatumHash),
    Inline(MemoizedPlutusData),
}

impl From<Option<MintedDatumOption<'_>>> for MemoizedDatum {
    fn from(opt: Option<MintedDatumOption<'_>>) -> Self {
        match opt {
            None => Self::None,
            Some(MintedDatumOption::Hash(hash)) => Self::Hash(hash),
            Some(MintedDatumOption::Data(cbor_wrap)) => {
                let data = cbor_wrap.unwrap();
                Self::Inline(MemoizedPlutusData {
                    original_bytes: Bytes::from(data.raw_cbor().to_vec()),
                    data: data.unwrap(),
                })
            }
        }
    }
}

impl From<Option<DatumHash>> for MemoizedDatum {
    fn from(opt: Option<DatumHash>) -> Self {
        opt.map(MemoizedDatum::Hash).unwrap_or(MemoizedDatum::None)
    }
}

// ---------------------------------------------------------- MemoizedPlutusData

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct MemoizedPlutusData {
    original_bytes: Bytes,
    pub data: PlutusData,
}

impl MemoizedPlutusData {
    pub fn original_bytes(&self) -> &[u8] {
        &self.original_bytes
    }

    pub fn hash(&self) -> Hash<32> {
        Hasher::<256>::hash(&self.original_bytes)
    }
}

impl TryFrom<Vec<u8>> for MemoizedPlutusData {
    type Error = cbor::decode::Error;

    fn try_from(bytes: Vec<u8>) -> Result<Self, Self::Error> {
        let data = cbor::decode(&bytes)?;
        Ok(Self {
            original_bytes: Bytes::from(bytes),
            data,
        })
    }
}

// -------------------------------------------------------- MemoizedNativeScript

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct MemoizedNativeScript {
    original_bytes: Bytes,
    pub expr: NativeScript,
}

impl MemoizedNativeScript {
    pub fn original_bytes(&self) -> &[u8] {
        &self.original_bytes
    }
}

impl From<KeepRaw<'_, NativeScript>> for MemoizedNativeScript {
    fn from(script: KeepRaw<'_, NativeScript>) -> Self {
        MemoizedNativeScript {
            original_bytes: Bytes::from(script.raw_cbor().to_vec()),
            expr: script.unwrap(),
        }
    }
}
