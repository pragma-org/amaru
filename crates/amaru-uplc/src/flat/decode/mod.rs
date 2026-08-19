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

use amaru_kernel::{PlutusScript, PlutusVersion, ProtocolVersion, ToBytes, reify_plutus_version};
use bumpalo::collections::Vec as BumpVec;
use num::Zero;

use super::{
    tag,
    tag::{BUILTIN_TAG_WIDTH, CONST_TAG_WIDTH, TERM_TAG_WIDTH},
};
use crate::{
    arena::Arena,
    binder::{Binder, DeBruijn},
    builtin::DefaultFunction,
    constant::Constant,
    ledger_value::{CurrencyEntry, LedgerValue, TokenEntry, check_quantity_range, count_stats},
    machine::MachineVersion,
    program::Program,
    term::Term,
    typ::Type,
};

mod decoder;
pub use decoder::{Ctx, Decoder, SimpleCtx};

mod error;
pub use error::FlatDecodeError;

/// Decode a 'DeBruijn' UPLC program from encoded flat bytes.
pub fn decode_plutus_script<'a, const V: usize>(
    script: &PlutusScript<V>,
    protocol_version: ProtocolVersion,
    arena: &'a Arena,
) -> Result<(&'a Program<'a, DeBruijn>, PlutusVersion), FlatDecodeError> {
    let bytes = script.to_bytes().map_err(|e| {
        FlatDecodeError::Message(format!("unable to get raw flat bytes: error={e}, script={script:#?}"))
    })?;

    // TODO: carry IsKnownPlutusVersion constraint
    //
    // We should carry the `IsKnownPlutusVersion` constraint up until here if possible, so
    // that this conversion can be infaillible. This means that upon successfully decoding a
    // transaction, we should instantiate the constraint and carry it through; until that is too
    // cumbersome.
    let plutus_version = reify_plutus_version::<V>()
        .ok_or_else(|| FlatDecodeError::Message(format!("unable to reify type-level Plutus version '{V:#?}' ??!")))?;

    let (program, remainder) = decode(arena, bytes, protocol_version)?;

    if plutus_version >= PlutusVersion::V3 && remainder > 0 {
        return Err(FlatDecodeError::TrailingBytes(remainder));
    }

    Ok((program, plutus_version))
}

/// Decode a FLAT-encoded program according to a specific protocol version.
///
/// CONSTR/CASE terms, certain constants and certain builtins are rejected when the program version
/// or protocol version combination disallows them.
pub fn decode<'a, V>(
    arena: &'a Arena,
    bytes: &[u8],
    protocol_version: ProtocolVersion,
) -> Result<(&'a Program<'a, V>, usize), FlatDecodeError>
where
    V: Binder<'a>,
{
    let mut decoder = Decoder::new(bytes);

    let major = decoder.word()?;
    let minor = decoder.word()?;
    let patch = decoder.word()?;
    let machine_version = MachineVersion::new(major, minor, patch);

    let mut ctx = Ctx { arena, machine_version, protocol_version };

    let term = decode_term(&mut ctx, &mut decoder)?;

    decoder.filler()?;

    let remainder = decoder.buffer.len() - decoder.pos;

    Ok((Program::new(arena, machine_version, term), remainder))
}

fn decode_term<'a, V>(ctx: &mut Ctx<'a>, decoder: &mut Decoder<'_>) -> Result<&'a Term<'a, V>, FlatDecodeError>
where
    V: Binder<'a>,
{
    let tag = decoder.bits8(TERM_TAG_WIDTH)?;

    match tag {
        // Var
        tag::VAR => Ok(Term::var(ctx.arena, V::var_decode(ctx.arena, decoder)?)),
        // Delay
        tag::DELAY => {
            let term = decode_term(ctx, decoder)?;

            Ok(term.delay(ctx.arena))
        }
        // Lambda
        tag::LAMBDA => {
            let param = V::parameter_decode(ctx.arena, decoder)?;

            let term = decode_term(ctx, decoder)?;

            Ok(term.lambda(ctx.arena, param))
        }
        // Apply
        tag::APPLY => {
            let function = decode_term(ctx, decoder)?;
            let argument = decode_term(ctx, decoder)?;

            let term = function.apply(ctx.arena, argument);

            Ok(term)
        }
        // Constant
        tag::CONSTANT => {
            let constant = decode_constant(ctx, decoder)?;

            Ok(Term::constant(ctx.arena, constant))
        }
        // Force
        tag::FORCE => {
            let term = decode_term(ctx, decoder)?;

            Ok(term.force(ctx.arena))
        }
        // Error
        tag::ERROR => Ok(Term::error(ctx.arena)),
        // Builtin
        tag::BUILTIN => {
            let builtin_tag = decoder.bits8(BUILTIN_TAG_WIDTH)?;

            let function = DefaultFunction::try_from(builtin_tag)
                .map_err(|()| FlatDecodeError::DefaultFunctionNotFound(builtin_tag))?;

            if !ctx.is_builtin_available(function) {
                return Err(FlatDecodeError::BuiltinNotAvailable(builtin_tag, format!("{function:?}")));
            }

            let term = Term::builtin(ctx.arena, function);

            Ok(term)
        }
        // Constr
        tag::CONSTR => {
            if !ctx.is_constr_case_available() {
                return Err(FlatDecodeError::TermNotAvailable(tag::CONSTR, "constr"));
            }

            let tag = decoder.word()?;
            let fields = decoder.list_with(ctx, decode_term)?;
            let fields = ctx.arena.alloc(fields);

            let term = Term::constr(ctx.arena, tag, fields);

            Ok(term)
        }
        // Case
        tag::CASE => {
            if !ctx.is_constr_case_available() {
                return Err(FlatDecodeError::TermNotAvailable(tag::CASE, "case"));
            }

            let constr = decode_term(ctx, decoder)?;
            let branches = decoder.list_with(ctx, decode_term)?;
            let branches = ctx.arena.alloc(branches);

            Ok(Term::case(ctx.arena, constr, branches))
        }
        _ => Err(FlatDecodeError::UnknownTermConstructor(tag)),
    }
}

fn type_from_tags<'a>(ctx: &Ctx<'a>, tags: &[u8]) -> Result<(&'a Type<'a>, usize), FlatDecodeError> {
    match tags {
        [tag::INTEGER, ..] => Ok((Type::integer(ctx.arena), 1)),
        [tag::BYTE_STRING, ..] => Ok((Type::byte_string(ctx.arena), 1)),
        [tag::STRING, ..] => Ok((Type::string(ctx.arena), 1)),
        [tag::UNIT, ..] => Ok((Type::unit(ctx.arena), 1)),
        [tag::BOOL, ..] => Ok((Type::bool(ctx.arena), 1)),
        [tag::DATA, ..] => Ok((Type::data(ctx.arena), 1)),
        [tag::PROTO_LIST_ONE, tag::PROTO_LIST_TWO, rest @ ..] => {
            let (sub_typ, consumed) = type_from_tags(ctx, rest)?;
            Ok((Type::list(ctx.arena, sub_typ), 2 + consumed))
        }
        [tag::PROTO_ARRAY_ONE, tag::PROTO_ARRAY_TWO, rest @ ..] => {
            let (sub_typ, consumed) = type_from_tags(ctx, rest)?;
            Ok((Type::array(ctx.arena, sub_typ), 2 + consumed))
        }
        [tag::PROTO_PAIR_ONE, tag::PROTO_PAIR_TWO, tag::PROTO_PAIR_THREE, rest @ ..] => {
            let (sub_typ1, consumed1) = type_from_tags(ctx, rest)?;
            let rest2 = &rest[consumed1..];
            let (sub_typ2, consumed2) = type_from_tags(ctx, rest2)?;

            Ok((Type::pair(ctx.arena, sub_typ1, sub_typ2), 3 + consumed1 + consumed2))
        }
        [tag::BLS12_381_G1_ELEMENT, ..] => Ok((Type::g1(ctx.arena), 1)),
        [tag::BLS12_381_G2_ELEMENT, ..] => Ok((Type::g2(ctx.arena), 1)),
        [tag::BLS12_381_ML_RESULT, ..] => Ok((Type::ml_result(ctx.arena), 1)),
        [tag::VALUE, ..] => Ok((Type::value(ctx.arena), 1)),
        [] => Err(FlatDecodeError::MissingTypeTag),
        x => Err(FlatDecodeError::UnknownTypeTags(x.to_vec())),
    }
}

// BLS literals not supported
fn decode_constant<'a>(ctx: &mut Ctx<'a>, d: &mut Decoder) -> Result<&'a Constant<'a>, FlatDecodeError> {
    let tags = decode_constant_tags(ctx, d)?;
    let (ty, _) = type_from_tags(ctx, tags.as_slice())?;

    match ty {
        Type::Integer => {
            let v = d.integer()?;
            let v = ctx.arena.alloc_integer(v);

            Ok(Constant::integer(ctx.arena, v))
        }
        Type::ByteString => {
            let b = d.bytes(ctx.arena)?;
            let b = ctx.arena.alloc(b);

            Ok(Constant::byte_string(ctx.arena, b))
        }
        Type::Bool => {
            let v = d.bit()?;

            Ok(Constant::bool(ctx.arena, v))
        }
        Type::String => {
            let s = d.utf8(ctx.arena)?;
            let s = ctx.arena.alloc(s);

            Ok(Constant::string(ctx.arena, s))
        }
        Type::Unit => Ok(Constant::unit(ctx.arena)),
        Type::List(sub_typ) => {
            let fields = d.list_with(ctx, |ctx, d| decode_constant_with_type(ctx, d, sub_typ))?;
            let fields = ctx.arena.alloc(fields);

            Ok(Constant::proto_list(ctx.arena, sub_typ, fields))
        }

        Type::Array(sub_typ) => {
            let fields = d.list_with(ctx, |ctx, d| decode_constant_with_type(ctx, d, sub_typ))?;
            let fields = ctx.arena.alloc(fields);
            Ok(Constant::proto_array(ctx.arena, sub_typ, fields))
        }
        Type::Pair(sub_typ1, sub_typ2) => {
            let fst = decode_constant_with_type(ctx, d, sub_typ1)?;
            let snd = decode_constant_with_type(ctx, d, sub_typ2)?;

            Ok(Constant::proto_pair(ctx.arena, sub_typ1, sub_typ2, fst, snd))
        }
        Type::Data => {
            let cbor = d.bytes(ctx.arena)?;
            let data = minicbor::decode_with(&cbor, &mut SimpleCtx { arena: ctx.arena })?;
            Ok(Constant::data(ctx.arena, data))
        }
        // BLS12-381 element *values* have no flat encoding: their `Flat` instances fail in the
        // Haskell implementation (plutus #5663), so a script carrying an actual BLS constant is
        // malformed there too. The *types* must still decode (see `type_from_tags`) so that
        // constants like `(con (list bls12_381_G1_element) [])`, which contain no element value,
        // deserialize exactly as they do on the Haskell side.
        Type::Bls12_381G1Element | Type::Bls12_381G2Element | Type::Bls12_381MlResult => {
            Err(FlatDecodeError::BlsValueNotSupported)
        }
        Type::Value => decode_value(ctx, d),
    }
}

// BLS literals not supported
fn decode_constant_with_type<'a>(
    ctx: &mut Ctx<'a>,
    d: &mut Decoder,
    ty: &Type<'a>,
) -> Result<&'a Constant<'a>, FlatDecodeError> {
    match ty {
        Type::Integer => {
            let v = d.integer()?;
            let v = ctx.arena.alloc_integer(v);

            Ok(Constant::integer(ctx.arena, v))
        }
        Type::ByteString => {
            let b = d.bytes(ctx.arena)?;
            let b = ctx.arena.alloc(b);

            Ok(Constant::byte_string(ctx.arena, b))
        }
        Type::Bool => {
            let v = d.bit()?;

            Ok(Constant::bool(ctx.arena, v))
        }
        Type::String => {
            let s = d.utf8(ctx.arena)?;
            let s = ctx.arena.alloc(s);

            Ok(Constant::string(ctx.arena, s))
        }
        Type::Unit => Ok(Constant::unit(ctx.arena)),
        Type::List(sub_typ) => {
            let fields = d.list_with(ctx, |ctx, d| decode_constant_with_type(ctx, d, sub_typ))?;
            let fields = ctx.arena.alloc(fields);

            Ok(Constant::proto_list(ctx.arena, sub_typ, fields))
        }
        Type::Array(sub_typ) => {
            let fields = d.list_with(ctx, |ctx, d| decode_constant_with_type(ctx, d, sub_typ))?;
            let fields = ctx.arena.alloc(fields);
            Ok(Constant::proto_array(ctx.arena, sub_typ, fields))
        }
        Type::Pair(sub_typ1, sub_typ2) => {
            let fst = decode_constant_with_type(ctx, d, sub_typ1)?;
            let snd = decode_constant_with_type(ctx, d, sub_typ2)?;

            Ok(Constant::proto_pair(ctx.arena, sub_typ1, sub_typ2, fst, snd))
        }
        Type::Data => {
            let cbor = d.bytes(ctx.arena)?;
            let data = minicbor::decode_with(&cbor, &mut SimpleCtx { arena: ctx.arena })?;

            Ok(Constant::data(ctx.arena, data))
        }
        // BLS12-381 element *values* have no flat encoding: their `Flat` instances fail in the
        // Haskell implementation (plutus #5663), so a script carrying an actual BLS constant is
        // malformed there too. The *types* must still decode (see `type_from_tags`) so that
        // constants like `(con (list bls12_381_G1_element) [])`, which contain no element value,
        // deserialize exactly as they do on the Haskell side.
        Type::Bls12_381G1Element | Type::Bls12_381G2Element | Type::Bls12_381MlResult => {
            Err(FlatDecodeError::BlsValueNotSupported)
        }
        Type::Value => decode_value(ctx, d),
    }
}

fn decode_value<'a>(ctx: &mut Ctx<'a>, d: &mut Decoder) -> Result<&'a Constant<'a>, FlatDecodeError> {
    let arena = ctx.arena;

    let mut currency_entries = BumpVec::new_in(arena.as_bump());
    let mut prev_ccy: Option<&[u8]> = None;

    // Outer map: bit-prefix list of (ByteString, Map ByteString Integer)
    while d.bit()? {
        let ccy = d.bytes(arena)?;

        if ccy.len() > 32 {
            return Err(FlatDecodeError::Message("Value key exceeds 32 bytes".into()));
        }

        let ccy: &'a [u8] = arena.alloc(ccy);

        // Currency symbols must be strictly ascending
        if let Some(prev) = prev_ccy
            && prev >= ccy
        {
            return Err(FlatDecodeError::Message("Value currency symbols not strictly ascending".into()));
        }
        prev_ccy = Some(ccy);

        let mut token_entries = BumpVec::new_in(arena.as_bump());
        let mut prev_tok: Option<&[u8]> = None;

        // Inner map: bit-prefix list of (ByteString, Integer)
        while d.bit()? {
            let tok = d.bytes(arena)?;

            if tok.len() > 32 {
                return Err(FlatDecodeError::Message("Value token name exceeds 32 bytes".into()));
            }

            let tok: &'a [u8] = arena.alloc(tok);

            // Token names must be strictly ascending
            if let Some(prev) = prev_tok
                && prev >= tok
            {
                return Err(FlatDecodeError::Message("Value token names not strictly ascending".into()));
            }
            prev_tok = Some(tok);

            let qty = d.integer()?;

            if check_quantity_range(&qty).is_err() {
                return Err(FlatDecodeError::Message("Value quantity out of range".into()));
            }

            // No zero quantities
            if qty.is_zero() {
                return Err(FlatDecodeError::Message("Value contains zero quantity".into()));
            }

            let qty = arena.alloc_integer(qty);

            token_entries.push(TokenEntry { name: tok, quantity: qty });
        }

        let tokens: &'a [TokenEntry<'a>] = arena.alloc(token_entries);

        // No empty inner maps
        if tokens.is_empty() {
            return Err(FlatDecodeError::Message("Value contains empty inner map".into()));
        }

        currency_entries.push(CurrencyEntry { currency: ccy, tokens });
    }

    let entries: &'a [CurrencyEntry<'a>] = arena.alloc(currency_entries);
    let (size, negative_count) = count_stats(entries);

    let v = arena.alloc(LedgerValue { entries, size, negative_count });

    Ok(Constant::ledger_value(arena, v))
}

fn decode_constant_tags<'a>(ctx: &mut Ctx<'a>, d: &mut Decoder) -> Result<BumpVec<'a, u8>, FlatDecodeError> {
    d.list_with(ctx, |_arena, d| decode_constant_tag(d))
}

fn decode_constant_tag(d: &mut Decoder) -> Result<u8, FlatDecodeError> {
    d.bits8(CONST_TAG_WIDTH)
}

#[cfg(test)]
mod tests {
    use amaru_kernel::PROTOCOL_VERSION_10;
    use hex;
    use num::BigInt;

    use super::*;
    use crate::{arena::Arena, binder::DeBruijn};

    #[test]
    fn decode_program_big_constr_tag() {
        // (program 1.1.0
        //   [
        //     [
        //       (builtin addInteger)
        //       (con integer 1)
        //     ]
        //     [ (force (force (builtin fstPair)))
        //       [ (builtin unConstrData)
        //         (con data (Constr 128 [I 0, I 1]))
        //       ]
        //     ]
        //   ])
        let bytes = hex::decode("0101003370090011aab9d375498109d8668218809f0001ff0001").unwrap();
        let arena = Arena::new();
        let program: Result<(&Program<DeBruijn>, _), _> = decode(&arena, &bytes, PROTOCOL_VERSION_10);
        match program {
            Ok((program, _)) => {
                let eval_result = program.eval_default(&arena);
                let term = eval_result.term.unwrap();
                assert_eq!(term, &Term::Constant(&Constant::Integer(&BigInt::from(129))));
            }
            Err(_) => {
                panic!();
            }
        }
    }

    #[test]
    fn decode_program_bigint() {
        // (program 1.1.0
        //   [
        //     [
        //       (builtin addInteger)
        //       (con integer 1)
        //     ]
        //     [ (builtin unIData)
        //       [ (force (builtin headList))
        //         [ (force (force (builtin sndPair)))
        //           [ (builtin unConstrData)
        //             (con data (Constr 0 [I 999999999999999999999999999]))
        //           ]
        //         ]
        //       ]
        //     ]
        //   ])
        let bytes =
            hex::decode("0101003370090011bad357426aae78dd526112d8799fc24c033b2e3c9fd0803ce7ffffffff0001").unwrap();
        let arena = Arena::new();
        let program: Result<(&Program<DeBruijn>, _), _> = decode(&arena, &bytes, PROTOCOL_VERSION_10);
        match program {
            Ok((program, _)) => {
                let eval_result = program.eval_default(&arena);
                let term = eval_result.term.unwrap();
                assert_eq!(
                    term,
                    &Term::Constant(&Constant::Integer(&BigInt::from(1_000_000_000_000_000_000_000_000_000i128)))
                );
            }
            Err(e) => {
                panic!("{}", e);
            }
        }
    }

    #[test]
    fn decode_program_list() {
        // (program 1.1.0
        //   [
        //     [
        //       (builtin multiplyInteger)
        //       (con integer 2)
        //     ]
        //     [ (builtin unIData)
        //       [ (force (builtin headList))
        //         [ (force (builtin tailList))
        //           [ (builtin unListData)
        //             (con data (List [I 7, I 14]))
        //           ]
        //         ]
        //       ]
        //     ]
        //   ])
        let bytes = hex::decode("0101003370490021bad357426ae88dd62601049f070eff0001").unwrap();
        let arena = Arena::new();
        let program: Result<(&Program<DeBruijn>, _), _> = decode(&arena, &bytes, PROTOCOL_VERSION_10);
        match program {
            Ok((program, _)) => {
                let eval_result = program.eval_default(&arena);
                let term = eval_result.term.unwrap();
                assert_eq!(term, &Term::Constant(&Constant::Integer(&BigInt::from(28))));
            }
            Err(e) => {
                panic!("{}", e);
            }
        }
    }
}
