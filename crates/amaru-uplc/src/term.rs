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
    arena::Arena,
    builtin::DefaultFunction,
    constant::{Constant, Integer, integer_from},
    data::PlutusData,
};

#[derive(Debug, PartialEq, Clone)]
pub enum Term<'a, V> {
    Var(&'a V),

    Lambda {
        parameter: &'a V,
        body: &'a Term<'a, V>,
    },

    Apply {
        function: &'a Term<'a, V>,
        argument: &'a Term<'a, V>,
    },

    Delay(&'a Term<'a, V>),

    Force(&'a Term<'a, V>),

    Case {
        constr: &'a Term<'a, V>,
        branches: &'a [&'a Term<'a, V>],
    },

    Constr {
        // TODO: revisit what the best type is for this
        tag: usize,
        fields: &'a [&'a Term<'a, V>],
    },

    Constant(&'a Constant<'a>),

    Builtin(DefaultFunction),

    Error,
}

impl<'a, V> Term<'a, V> {
    pub fn var(arena: &'a Arena, i: &'a V) -> &'a Term<'a, V> {
        arena.alloc(Term::Var(i))
    }

    pub fn apply(&'a self, arena: &'a Arena, argument: &'a Term<'a, V>) -> &'a Term<'a, V> {
        arena.alloc(Term::Apply { function: self, argument })
    }

    pub fn lambda(&'a self, arena: &'a Arena, parameter: &'a V) -> &'a Term<'a, V> {
        arena.alloc(Term::Lambda { parameter, body: self })
    }

    pub fn force(&'a self, arena: &'a Arena) -> &'a Term<'a, V> {
        arena.alloc(Term::Force(self))
    }

    pub fn delay(&'a self, arena: &'a Arena) -> &'a Term<'a, V> {
        arena.alloc(Term::Delay(self))
    }

    pub fn constant(arena: &'a Arena, constant: &'a Constant<'a>) -> &'a Term<'a, V> {
        arena.alloc(Term::Constant(constant))
    }

    pub fn constr(arena: &'a Arena, tag: usize, fields: &'a [&'a Term<'a, V>]) -> &'a Term<'a, V> {
        arena.alloc(Term::Constr { tag, fields })
    }

    pub fn case(arena: &'a Arena, constr: &'a Term<'a, V>, branches: &'a [&'a Term<'a, V>]) -> &'a Term<'a, V> {
        arena.alloc(Term::Case { constr, branches })
    }

    pub fn integer(arena: &'a Arena, i: &'a Integer) -> &'a Term<'a, V> {
        let constant = arena.alloc(Constant::Integer(i));

        Term::constant(arena, constant)
    }

    pub fn integer_from(arena: &'a Arena, i: i128) -> &'a Term<'a, V> {
        Self::integer(arena, integer_from(arena, i))
    }

    pub fn byte_string(arena: &'a Arena, bytes: &'a [u8]) -> &'a Term<'a, V> {
        let constant = Constant::byte_string(arena, bytes);

        Term::constant(arena, constant)
    }

    pub fn string(arena: &'a Arena, s: &'a str) -> &'a Term<'a, V> {
        let constant = Constant::string(arena, s);

        Term::constant(arena, constant)
    }

    pub fn bool(arena: &'a Arena, v: bool) -> &'a Term<'a, V> {
        let constant = Constant::bool(arena, v);

        Term::constant(arena, constant)
    }

    pub fn data(arena: &'a Arena, d: &'a PlutusData<'a>) -> &'a Term<'a, V> {
        let constant = Constant::data(arena, d);

        Term::constant(arena, constant)
    }

    pub fn data_byte_string(arena: &'a Arena, bytes: &'a [u8]) -> &'a Term<'a, V> {
        let data = PlutusData::byte_string(arena, bytes);

        Term::data(arena, data)
    }

    pub fn data_integer(arena: &'a Arena, i: &'a Integer) -> &'a Term<'a, V> {
        let data = PlutusData::integer(arena, i);

        Term::data(arena, data)
    }

    pub fn data_integer_from(arena: &'a Arena, i: i128) -> &'a Term<'a, V> {
        let data = PlutusData::integer_from(arena, i);

        Term::data(arena, data)
    }

    pub fn unit(arena: &'a Arena) -> &'a Term<'a, V> {
        let constant = Constant::unit(arena);

        Term::constant(arena, constant)
    }

    pub fn builtin(arena: &'a Arena, fun: DefaultFunction) -> &'a Term<'a, V> {
        arena.alloc(Term::Builtin(fun))
    }

    pub fn error(arena: &'a Arena) -> &'a Term<'a, V> {
        arena.alloc(Term::Error)
    }

    pub fn add_integer(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::AddInteger)
    }

    pub fn multiply_integer(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::MultiplyInteger)
    }

    pub fn divide_integer(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::DivideInteger)
    }

    pub fn quotient_integer(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::QuotientInteger)
    }

    pub fn remainder_integer(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::RemainderInteger)
    }

    pub fn mod_integer(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::ModInteger)
    }

    pub fn subtract_integer(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::SubtractInteger)
    }

    pub fn equals_integer(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::EqualsInteger)
    }

    pub fn less_than_equals_integer(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::LessThanEqualsInteger)
    }

    pub fn less_than_integer(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::LessThanInteger)
    }

    pub fn if_then_else(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::IfThenElse)
    }

    pub fn append_byte_string(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::AppendByteString)
    }

    pub fn equals_byte_string(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::EqualsByteString)
    }

    pub fn cons_byte_string(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::ConsByteString)
    }

    pub fn slice_byte_string(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::SliceByteString)
    }

    pub fn length_of_byte_string(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::LengthOfByteString)
    }

    pub fn index_byte_string(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::IndexByteString)
    }

    pub fn less_than_byte_string(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::LessThanByteString)
    }

    pub fn less_than_equals_byte_string(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::LessThanEqualsByteString)
    }

    pub fn sha2_256(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::Sha2_256)
    }

    pub fn sha3_256(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::Sha3_256)
    }

    pub fn blake2b_256(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::Blake2b_256)
    }

    pub fn keccak_256(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::Keccak_256)
    }

    pub fn blake2b_224(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::Blake2b_224)
    }

    pub fn verify_ed25519_signature(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::VerifyEd25519Signature)
    }

    pub fn verify_ecdsa_secp256k1_signature(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::VerifyEcdsaSecp256k1Signature)
    }

    pub fn verify_schnorr_secp256k1_signature(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::VerifySchnorrSecp256k1Signature)
    }

    pub fn append_string(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::AppendString)
    }

    pub fn equals_string(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::EqualsString)
    }

    pub fn encode_utf8(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::EncodeUtf8)
    }

    pub fn decode_utf8(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::DecodeUtf8)
    }

    pub fn choose_unit(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::ChooseUnit)
    }

    pub fn trace(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::Trace)
    }

    pub fn fst_pair(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::FstPair)
    }

    pub fn snd_pair(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::SndPair)
    }

    pub fn choose_list(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::ChooseList)
    }

    pub fn mk_cons(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::MkCons)
    }

    pub fn head_list(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::HeadList)
    }

    pub fn tail_list(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::TailList)
    }

    pub fn null_list(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::NullList)
    }

    pub fn choose_data(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::ChooseData)
    }

    pub fn constr_data(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::ConstrData)
    }

    pub fn map_data(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::MapData)
    }

    pub fn list_data(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::ListData)
    }

    pub fn i_data(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::IData)
    }

    pub fn b_data(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::BData)
    }

    pub fn un_constr_data(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::UnConstrData)
    }

    pub fn un_map_data(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::UnMapData)
    }

    pub fn un_list_data(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::UnListData)
    }

    pub fn un_i_data(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::UnIData)
    }

    pub fn un_b_data(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::UnBData)
    }

    pub fn equals_data(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::EqualsData)
    }

    pub fn mk_pair_data(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::MkPairData)
    }

    pub fn mk_nil_data(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::MkNilData)
    }

    pub fn mk_nil_pair_data(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::MkNilPairData)
    }

    pub fn serialise_data(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::SerialiseData)
    }

    pub fn bls12_381_g1_add(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::Bls12_381_G1_Add)
    }
    pub fn bls12_381_g1_neg(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::Bls12_381_G1_Neg)
    }
    pub fn bls12_381_g1_scalar_mul(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::Bls12_381_G1_ScalarMul)
    }
    pub fn bls12_381_g1_equal(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::Bls12_381_G1_Equal)
    }
    pub fn bls12_381_g1_compress(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::Bls12_381_G1_Compress)
    }
    pub fn bls12_381_g1_uncompress(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::Bls12_381_G1_Uncompress)
    }
    pub fn bls12_381_g1_hash_to_group(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::Bls12_381_G1_HashToGroup)
    }
    pub fn bls12_381_g2_add(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::Bls12_381_G2_Add)
    }
    pub fn bls12_381_g2_neg(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::Bls12_381_G2_Neg)
    }
    pub fn bls12_381_g2_scalar_mul(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::Bls12_381_G2_ScalarMul)
    }
    pub fn bls12_381_g2_equal(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::Bls12_381_G2_Equal)
    }
    pub fn bls12_381_g2_compress(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::Bls12_381_G2_Compress)
    }
    pub fn bls12_381_g2_uncompress(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::Bls12_381_G2_Uncompress)
    }
    pub fn bls12_381_g2_hash_to_group(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::Bls12_381_G2_HashToGroup)
    }
    pub fn bls12_381_miller_loop(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::Bls12_381_MillerLoop)
    }
    pub fn bls12_381_mul_ml_result(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::Bls12_381_MulMlResult)
    }
    pub fn bls12_381_final_verify(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::Bls12_381_FinalVerify)
    }
    pub fn integer_to_byte_string(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::IntegerToByteString)
    }
    pub fn byte_string_to_integer(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::ByteStringToInteger)
    }
    pub fn and_byte_string(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::AndByteString)
    }
    pub fn or_byte_string(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::OrByteString)
    }
    pub fn xor_byte_string(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::XorByteString)
    }
    pub fn complement_byte_string(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::ComplementByteString)
    }
    pub fn read_bit(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::ReadBit)
    }
    pub fn write_bits(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::WriteBits)
    }
    pub fn replicate_byte(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::ReplicateByte)
    }
    pub fn shift_byte_string(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::ShiftByteString)
    }
    pub fn rotate_byte_string(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::RotateByteString)
    }
    pub fn count_set_bits(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::CountSetBits)
    }
    pub fn find_first_set_bit(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::FindFirstSetBit)
    }
    pub fn ripemd_160(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::Ripemd_160)
    }

    pub fn exp_mod_integer(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::ExpModInteger)
    }

    pub fn drop_list(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::DropList)
    }

    pub fn length_of_array(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::LengthOfArray)
    }

    pub fn list_to_array(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::ListToArray)
    }

    pub fn index_array(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::IndexArray)
    }

    pub fn bls12_381_g1_multi_scalar_mul(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::Bls12_381_G1_MultiScalarMul)
    }

    pub fn bls12_381_g2_multi_scalar_mul(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::Bls12_381_G2_MultiScalarMul)
    }

    pub fn insert_coin(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::InsertCoin)
    }

    pub fn lookup_coin(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::LookupCoin)
    }

    pub fn union_value(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::UnionValue)
    }

    pub fn value_contains(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::ValueContains)
    }

    pub fn value_data(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::ValueData)
    }

    pub fn un_value_data(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::UnValueData)
    }

    pub fn scale_value(arena: &'a Arena) -> &'a Term<'a, V> {
        Term::builtin(arena, DefaultFunction::ScaleValue)
    }
}
