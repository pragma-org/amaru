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

#[allow(clippy::disallowed_types)]
use std::collections::HashMap;

use amaru_kernel::{PROTOCOL_VERSION_10, ProtocolVersion, protocol_version::PROTOCOL_VERSION_11};

use crate::{
    builtin::DefaultFunction,
    machine::{
        CostModel, ExBudget, Semantics,
        cost_model::{
            ParamName,
            costing::{
                AddedSizes, ConstantOrLinear, Cost, Costing, ExpModCost, FourArguments, LinearSize, MaxSize, MinSize,
                MultipliedSizes, OneArgument, QuadraticFunction, SixArguments, SubtractedSizes, ThreeArguments,
                TwoArguments, TwoArgumentsQuadraticFunction, TwoVariableLinearSize, WithInteraction,
            },
        },
    },
};

#[derive(Debug, PartialEq)]
pub struct BuiltinCosts {
    // Tracing
    pub trace: Costing<2, TwoArguments>,
    // Unit
    pub choose_unit: Costing<2, TwoArguments>,
    // Integers
    pub add_integer: Costing<2, TwoArguments>,
    pub divide_integer: Costing<2, TwoArguments>,
    pub equals_integer: Costing<2, TwoArguments>,
    pub integer_to_byte_string: Costing<3, ThreeArguments>,
    pub less_than_equals_integer: Costing<2, TwoArguments>,
    pub less_than_integer: Costing<2, TwoArguments>,
    pub mod_integer: Costing<2, TwoArguments>,
    pub multiply_integer: Costing<2, TwoArguments>,
    pub quotient_integer: Costing<2, TwoArguments>,
    pub remainder_integer: Costing<2, TwoArguments>,
    pub subtract_integer: Costing<2, TwoArguments>,
    // Bytestrings
    pub and_byte_string: Costing<3, ThreeArguments>,
    pub append_byte_string: Costing<2, TwoArguments>,
    pub byte_string_to_integer: Costing<2, TwoArguments>,
    pub complement_byte_string: Costing<1, OneArgument>,
    pub cons_byte_string: Costing<2, TwoArguments>,
    pub equals_byte_string: Costing<2, TwoArguments>,
    pub index_byte_string: Costing<2, TwoArguments>,
    pub length_of_byte_string: Costing<1, OneArgument>,
    pub less_than_byte_string: Costing<2, TwoArguments>,
    pub less_than_equals_byte_string: Costing<2, TwoArguments>,
    pub or_byte_string: Costing<3, ThreeArguments>,
    pub rotate_byte_string: Costing<2, TwoArguments>,
    pub shift_byte_string: Costing<2, TwoArguments>,
    pub slice_byte_string: Costing<3, ThreeArguments>,
    pub xor_byte_string: Costing<3, ThreeArguments>,
    // Bitwise
    pub read_bit: Costing<2, TwoArguments>,
    pub write_bits: Costing<3, ThreeArguments>,
    pub replicate_byte: Costing<2, TwoArguments>,
    pub count_set_bits: Costing<1, OneArgument>,
    pub find_first_set_bit: Costing<1, OneArgument>,
    // Strings
    pub append_string: Costing<2, TwoArguments>,
    pub equals_string: Costing<2, TwoArguments>,
    pub encode_utf8: Costing<1, OneArgument>,
    pub decode_utf8: Costing<1, OneArgument>,
    // Bool
    pub if_then_else: Costing<3, ThreeArguments>,
    // Value
    pub insert_coin: Costing<4, FourArguments>,
    pub lookup_coin: Costing<3, ThreeArguments>,
    pub scale_value: Costing<2, TwoArguments>,
    pub un_value_data: Costing<1, OneArgument>,
    pub union_value: Costing<2, TwoArguments>,
    pub value_contains: Costing<2, TwoArguments>,
    pub value_data: Costing<1, OneArgument>,
    // Lists
    pub choose_list: Costing<3, ThreeArguments>,
    pub drop_list: Costing<2, TwoArguments>,
    pub head_list: Costing<1, OneArgument>,
    pub mk_cons: Costing<2, TwoArguments>,
    pub null_list: Costing<1, OneArgument>,
    pub tail_list: Costing<1, OneArgument>,
    // Array
    pub index_array: Costing<2, TwoArguments>,
    pub length_of_array: Costing<1, OneArgument>,
    pub list_to_array: Costing<1, OneArgument>,
    // Pairs
    pub fst_pair: Costing<1, OneArgument>,
    pub snd_pair: Costing<1, OneArgument>,
    // Data
    pub b_data: Costing<1, OneArgument>,
    pub choose_data: Costing<6, SixArguments>,
    pub constr_data: Costing<2, TwoArguments>,
    pub equals_data: Costing<2, TwoArguments>,
    pub i_data: Costing<1, OneArgument>,
    pub list_data: Costing<1, OneArgument>,
    pub map_data: Costing<1, OneArgument>,
    pub mk_nil_data: Costing<1, OneArgument>,
    pub mk_nil_pair_data: Costing<1, OneArgument>,
    pub mk_pair_data: Costing<2, TwoArguments>,
    pub serialise_data: Costing<1, OneArgument>,
    pub un_b_data: Costing<1, OneArgument>,
    pub un_constr_data: Costing<1, OneArgument>,
    pub un_i_data: Costing<1, OneArgument>,
    pub un_list_data: Costing<1, OneArgument>,
    pub un_map_data: Costing<1, OneArgument>,
    // BLST
    pub bls12_381_final_verify: Costing<2, TwoArguments>,
    pub bls12_381_g1_add: Costing<2, TwoArguments>,
    pub bls12_381_g1_compress: Costing<1, OneArgument>,
    pub bls12_381_g1_equal: Costing<2, TwoArguments>,
    pub bls12_381_g1_hash_to_group: Costing<2, TwoArguments>,
    pub bls12_381_g1_multi_scalar_mul: Costing<2, TwoArguments>,
    pub bls12_381_g1_neg: Costing<1, OneArgument>,
    pub bls12_381_g1_scalar_mul: Costing<2, TwoArguments>,
    pub bls12_381_g1_uncompress: Costing<1, OneArgument>,
    pub bls12_381_g2_add: Costing<2, TwoArguments>,
    pub bls12_381_g2_compress: Costing<1, OneArgument>,
    pub bls12_381_g2_equal: Costing<2, TwoArguments>,
    pub bls12_381_g2_hash_to_group: Costing<2, TwoArguments>,
    pub bls12_381_g2_multi_scalar_mul: Costing<2, TwoArguments>,
    pub bls12_381_g2_neg: Costing<1, OneArgument>,
    pub bls12_381_g2_scalar_mul: Costing<2, TwoArguments>,
    pub bls12_381_g2_uncompress: Costing<1, OneArgument>,
    pub bls12_381_miller_loop: Costing<2, TwoArguments>,
    pub bls12_381_mul_ml_result: Costing<2, TwoArguments>,
    // Cryptography
    pub blake2b_224: Costing<1, OneArgument>,
    pub blake2b_256: Costing<1, OneArgument>,
    pub exp_mod_integer: Costing<3, ThreeArguments>,
    pub keccak_256: Costing<1, OneArgument>,
    pub ripemd_160: Costing<1, OneArgument>,
    pub sha2_256: Costing<1, OneArgument>,
    pub sha3_256: Costing<1, OneArgument>,
    pub verify_ecdsa_secp256k1_signature: Costing<3, ThreeArguments>,
    pub verify_ed25519_signature: Costing<3, ThreeArguments>,
    pub verify_schnorr_secp256k1_signature: Costing<3, ThreeArguments>,
}

impl Default for BuiltinCosts {
    fn default() -> Self {
        CostModel::v3().builtin_costs
    }
}

impl BuiltinCosts {
    #[allow(clippy::disallowed_types)]
    pub fn new(
        cost_map: &HashMap<ParamName, i64>,
        semantics: Semantics,
        protocol_version: ProtocolVersion,
    ) -> Result<Self, ParamName> {
        use ParamName::*;

        let always = |name: ParamName| cost_map.get(&name).copied().ok_or(name);

        let if_pv10: Box<dyn Fn(ParamName) -> Result<i64, ParamName>> = if protocol_version >= PROTOCOL_VERSION_10 {
            Box::new(always)
        } else {
            Box::new(|_name: ParamName| Ok(i64::MAX))
        };

        let if_pv11: Box<dyn Fn(ParamName) -> Result<i64, ParamName>> = if protocol_version >= PROTOCOL_VERSION_11 {
            Box::new(always)
        } else {
            Box::new(|_name: ParamName| Ok(i64::MAX))
        };

        Ok(Self {
            add_integer: Costing {
                mem: TwoArguments::MaxSize(MaxSize {
                    intercept: always(AddIntegerMemIntercept)?,
                    slope: always(AddIntegerMemSlope)?,
                }),
                cpu: TwoArguments::MaxSize(MaxSize {
                    intercept: always(AddIntegerCpuIntercept)?,
                    slope: always(AddIntegerCpuSlope)?,
                }),
            },
            append_byte_string: Costing {
                mem: TwoArguments::AddedSizes(AddedSizes {
                    intercept: always(AppendByteStringMemIntercept)?,
                    slope: always(AppendByteStringMemSlope)?,
                }),
                cpu: TwoArguments::AddedSizes(AddedSizes {
                    intercept: always(AppendByteStringCpuIntercept)?,
                    slope: always(AppendByteStringCpuSlope)?,
                }),
            },
            append_string: Costing {
                mem: TwoArguments::AddedSizes(AddedSizes {
                    intercept: always(AppendStringMemIntercept)?,
                    slope: always(AppendStringMemSlope)?,
                }),
                cpu: TwoArguments::AddedSizes(AddedSizes {
                    intercept: always(AppendStringCpuIntercept)?,
                    slope: always(AppendStringCpuSlope)?,
                }),
            },
            b_data: Costing {
                mem: OneArgument::Constant(always(BDataMem)?),
                cpu: OneArgument::Constant(always(BDataCpu)?),
            },
            blake2b_256: Costing {
                mem: OneArgument::Constant(always(Blake2b256Mem)?),
                cpu: OneArgument::LinearInX(LinearSize {
                    intercept: always(Blake2b256CpuIntercept)?,
                    slope: always(Blake2b256CpuSlope)?,
                }),
            },
            choose_data: Costing {
                mem: SixArguments::Constant(always(ChooseDataMem)?),
                cpu: SixArguments::Constant(always(ChooseDataCpu)?),
            },
            choose_list: Costing {
                mem: ThreeArguments::Constant(always(ChooseListMem)?),
                cpu: ThreeArguments::Constant(always(ChooseListCpu)?),
            },
            choose_unit: Costing {
                mem: TwoArguments::Constant(always(ChooseUnitMem)?),
                cpu: TwoArguments::Constant(always(ChooseUnitCpu)?),
            },
            cons_byte_string: Costing {
                mem: TwoArguments::AddedSizes(AddedSizes {
                    intercept: always(ConsByteStringMemIntercept)?,
                    slope: always(ConsByteStringMemSlope)?,
                }),
                cpu: TwoArguments::LinearInY(LinearSize {
                    intercept: always(ConsByteStringCpuIntercept)?,
                    slope: always(ConsByteStringCpuSlope)?,
                }),
            },
            constr_data: Costing {
                mem: TwoArguments::Constant(always(ConstrDataMem)?),
                cpu: TwoArguments::Constant(always(ConstrDataCpu)?),
            },
            decode_utf8: Costing {
                mem: OneArgument::LinearInX(LinearSize {
                    intercept: always(DecodeUtf8MemIntercept)?,
                    slope: always(DecodeUtf8MemSlope)?,
                }),
                cpu: OneArgument::LinearInX(LinearSize {
                    intercept: always(DecodeUtf8CpuIntercept)?,
                    slope: always(DecodeUtf8CpuSlope)?,
                }),
            },
            divide_integer: Costing {
                mem: TwoArguments::SubtractedSizes(SubtractedSizes {
                    intercept: always(DivideIntegerMemIntercept)?,
                    slope: always(DivideIntegerMemSlope)?,
                    minimum: always(DivideIntegerMemMinimum)?,
                }),
                cpu: match semantics {
                    Semantics::A | Semantics::B => TwoArguments::ConstAboveDiagonal(
                        always(DivideIntegerCpuConstant)?,
                        Box::new(TwoArguments::MultipliedSizes(MultipliedSizes {
                            intercept: always(DivideIntegerCpuIntercept)?,
                            slope: always(DivideIntegerCpuSlope)?,
                        })),
                    ),
                    Semantics::D => {
                        TwoArguments::AboveAndBelowDiagonal(Box::new(TwoArguments::MultipliedSizes(MultipliedSizes {
                            intercept: always(DivideIntegerCpuIntercept)?,
                            slope: always(DivideIntegerCpuSlope)?,
                        })))
                    }
                    Semantics::C => TwoArguments::ConstAboveDiagonal(
                        always(DivideIntegerCpuConstant)?,
                        Box::new(TwoArguments::QuadraticInXAndY(TwoArgumentsQuadraticFunction {
                            minimum: always(DivideIntegerCpuMinimum)?,
                            coeff_00: always(DivideIntegerCpuC00)?,
                            coeff_10: always(DivideIntegerCpuC10)?,
                            coeff_01: always(DivideIntegerCpuC01)?,
                            coeff_20: always(DivideIntegerCpuC20)?,
                            coeff_11: always(DivideIntegerCpuC11)?,
                            coeff_02: always(DivideIntegerCpuC02)?,
                        })),
                    ),
                    Semantics::E => TwoArguments::AboveAndBelowDiagonal(Box::new(TwoArguments::QuadraticInXAndY(
                        TwoArgumentsQuadraticFunction {
                            minimum: always(DivideIntegerCpuMinimum)?,
                            coeff_00: always(DivideIntegerCpuC00)?,
                            coeff_10: always(DivideIntegerCpuC10)?,
                            coeff_01: always(DivideIntegerCpuC01)?,
                            coeff_20: always(DivideIntegerCpuC20)?,
                            coeff_11: always(DivideIntegerCpuC11)?,
                            coeff_02: always(DivideIntegerCpuC02)?,
                        },
                    ))),
                },
            },
            encode_utf8: Costing {
                mem: OneArgument::LinearInX(LinearSize {
                    intercept: always(EncodeUtf8MemIntercept)?,
                    slope: always(EncodeUtf8MemSlope)?,
                }),
                cpu: OneArgument::LinearInX(LinearSize {
                    intercept: always(EncodeUtf8CpuIntercept)?,
                    slope: always(EncodeUtf8CpuSlope)?,
                }),
            },
            equals_byte_string: Costing {
                mem: TwoArguments::Constant(always(EqualsByteStringMem)?),
                cpu: TwoArguments::LinearOnDiagonal(ConstantOrLinear {
                    constant: always(EqualsByteStringCpuConstant)?,
                    intercept: always(EqualsByteStringCpuIntercept)?,
                    slope: always(EqualsByteStringCpuSlope)?,
                }),
            },
            equals_data: Costing {
                mem: TwoArguments::Constant(always(EqualsDataMem)?),
                cpu: TwoArguments::MinSize(MinSize {
                    intercept: always(EqualsDataCpuIntercept)?,
                    slope: always(EqualsDataCpuSlope)?,
                }),
            },
            equals_integer: Costing {
                mem: TwoArguments::Constant(always(EqualsIntegerMem)?),
                cpu: TwoArguments::MinSize(MinSize {
                    intercept: always(EqualsIntegerCpuIntercept)?,
                    slope: always(EqualsIntegerCpuSlope)?,
                }),
            },
            equals_string: Costing {
                mem: TwoArguments::Constant(always(EqualsStringMem)?),
                cpu: TwoArguments::LinearOnDiagonal(ConstantOrLinear {
                    constant: always(EqualsStringCpuConstant)?,
                    intercept: always(EqualsStringCpuIntercept)?,
                    slope: always(EqualsStringCpuSlope)?,
                }),
            },
            fst_pair: Costing {
                mem: OneArgument::Constant(always(FstPairMem)?),
                cpu: OneArgument::Constant(always(FstPairCpu)?),
            },
            head_list: Costing {
                mem: OneArgument::Constant(always(HeadListMem)?),
                cpu: OneArgument::Constant(always(HeadListCpu)?),
            },
            i_data: Costing {
                mem: OneArgument::Constant(always(IDataMem)?),
                cpu: OneArgument::Constant(always(IDataCpu)?),
            },
            if_then_else: Costing {
                mem: ThreeArguments::Constant(always(IfThenElseMem)?),
                cpu: ThreeArguments::Constant(always(IfThenElseCpu)?),
            },
            index_byte_string: Costing {
                mem: TwoArguments::Constant(always(IndexByteStringMem)?),
                cpu: TwoArguments::Constant(always(IndexByteStringCpu)?),
            },
            length_of_byte_string: Costing {
                mem: OneArgument::Constant(always(LengthOfByteStringMem)?),
                cpu: OneArgument::Constant(always(LengthOfByteStringCpu)?),
            },
            less_than_byte_string: Costing {
                mem: TwoArguments::Constant(always(LessThanByteStringMem)?),
                cpu: TwoArguments::MinSize(MinSize {
                    intercept: always(LessThanByteStringCpuIntercept)?,
                    slope: always(LessThanByteStringCpuSlope)?,
                }),
            },
            less_than_equals_byte_string: Costing {
                mem: TwoArguments::Constant(always(LessThanEqualsByteStringMem)?),
                cpu: TwoArguments::MinSize(MinSize {
                    intercept: always(LessThanEqualsByteStringCpuIntercept)?,
                    slope: always(LessThanEqualsByteStringCpuSlope)?,
                }),
            },
            less_than_equals_integer: Costing {
                mem: TwoArguments::Constant(always(LessThanEqualsIntegerMem)?),
                cpu: TwoArguments::MinSize(MinSize {
                    intercept: always(LessThanEqualsIntegerCpuIntercept)?,
                    slope: always(LessThanEqualsIntegerCpuSlope)?,
                }),
            },
            less_than_integer: Costing {
                mem: TwoArguments::Constant(always(LessThanIntegerMem)?),
                cpu: TwoArguments::MinSize(MinSize {
                    intercept: always(LessThanIntegerCpuIntercept)?,
                    slope: always(LessThanIntegerCpuSlope)?,
                }),
            },
            list_data: Costing {
                mem: OneArgument::Constant(always(ListDataMem)?),
                cpu: OneArgument::Constant(always(ListDataCpu)?),
            },
            map_data: Costing {
                mem: OneArgument::Constant(always(MapDataMem)?),
                cpu: OneArgument::Constant(always(MapDataCpu)?),
            },
            mk_cons: Costing {
                mem: TwoArguments::Constant(always(MkConsMem)?),
                cpu: TwoArguments::Constant(always(MkConsCpu)?),
            },
            mk_nil_data: Costing {
                mem: OneArgument::Constant(always(MkNilDataMem)?),
                cpu: OneArgument::Constant(always(MkNilDataCpu)?),
            },
            mk_nil_pair_data: Costing {
                mem: OneArgument::Constant(always(MkNilPairDataMem)?),
                cpu: OneArgument::Constant(always(MkNilPairDataCpu)?),
            },
            mk_pair_data: Costing {
                mem: TwoArguments::Constant(always(MkPairDataMem)?),
                cpu: TwoArguments::Constant(always(MkPairDataCpu)?),
            },
            mod_integer: Costing {
                mem: match semantics {
                    Semantics::A | Semantics::B => TwoArguments::SubtractedSizes(SubtractedSizes {
                        intercept: always(ModIntegerMemIntercept)?,
                        minimum: always(ModIntegerMemMinimum)?,
                        slope: always(ModIntegerMemSlope)?,
                    }),
                    Semantics::C | Semantics::D | Semantics::E => TwoArguments::LinearInY(LinearSize {
                        intercept: always(ModIntegerMemIntercept)?,
                        slope: always(ModIntegerMemSlope)?,
                    }),
                },
                cpu: match semantics {
                    Semantics::A | Semantics::B => TwoArguments::ConstAboveDiagonal(
                        always(ModIntegerCpuConstant)?,
                        Box::new(TwoArguments::MultipliedSizes(MultipliedSizes {
                            intercept: always(ModIntegerCpuIntercept)?,
                            slope: always(ModIntegerCpuSlope)?,
                        })),
                    ),
                    Semantics::D => {
                        TwoArguments::AboveAndBelowDiagonal(Box::new(TwoArguments::MultipliedSizes(MultipliedSizes {
                            intercept: always(ModIntegerCpuIntercept)?,
                            slope: always(ModIntegerCpuSlope)?,
                        })))
                    }
                    Semantics::C => TwoArguments::ConstAboveDiagonal(
                        always(ModIntegerCpuConstant)?,
                        Box::new(TwoArguments::QuadraticInXAndY(TwoArgumentsQuadraticFunction {
                            minimum: always(ModIntegerCpuMinimum)?,
                            coeff_00: always(ModIntegerCpuC00)?,
                            coeff_10: always(ModIntegerCpuC10)?,
                            coeff_01: always(ModIntegerCpuC01)?,
                            coeff_20: always(ModIntegerCpuC20)?,
                            coeff_11: always(ModIntegerCpuC11)?,
                            coeff_02: always(ModIntegerCpuC02)?,
                        })),
                    ),
                    Semantics::E => TwoArguments::AboveAndBelowDiagonal(Box::new(TwoArguments::QuadraticInXAndY(
                        TwoArgumentsQuadraticFunction {
                            minimum: always(ModIntegerCpuMinimum)?,
                            coeff_00: always(ModIntegerCpuC00)?,
                            coeff_10: always(ModIntegerCpuC10)?,
                            coeff_01: always(ModIntegerCpuC01)?,
                            coeff_20: always(ModIntegerCpuC20)?,
                            coeff_11: always(ModIntegerCpuC11)?,
                            coeff_02: always(ModIntegerCpuC02)?,
                        },
                    ))),
                },
            },
            multiply_integer: Costing {
                mem: TwoArguments::AddedSizes(AddedSizes {
                    intercept: always(MultiplyIntegerMemIntercept)?,
                    slope: always(MultiplyIntegerMemSlope)?,
                }),
                cpu: match semantics {
                    Semantics::A => TwoArguments::AddedSizes(AddedSizes {
                        intercept: always(MultiplyIntegerCpuIntercept)?,
                        slope: always(MultiplyIntegerCpuSlope)?,
                    }),
                    Semantics::B | Semantics::C | Semantics::D | Semantics::E => {
                        TwoArguments::MultipliedSizes(MultipliedSizes {
                            intercept: always(MultiplyIntegerCpuIntercept)?,
                            slope: always(MultiplyIntegerCpuSlope)?,
                        })
                    }
                },
            },
            null_list: Costing {
                mem: OneArgument::Constant(always(NullListMem)?),
                cpu: OneArgument::Constant(always(NullListCpu)?),
            },
            quotient_integer: Costing {
                mem: TwoArguments::SubtractedSizes(SubtractedSizes {
                    intercept: always(QuotientIntegerMemIntercept)?,
                    slope: always(QuotientIntegerMemSlope)?,
                    minimum: always(QuotientIntegerMemMinimum)?,
                }),
                cpu: match semantics {
                    Semantics::A | Semantics::B | Semantics::D => TwoArguments::ConstAboveDiagonal(
                        always(QuotientIntegerCpuConstant)?,
                        Box::new(TwoArguments::MultipliedSizes(MultipliedSizes {
                            intercept: always(QuotientIntegerCpuIntercept)?,
                            slope: always(QuotientIntegerCpuSlope)?,
                        })),
                    ),
                    Semantics::C | Semantics::E => TwoArguments::ConstAboveDiagonal(
                        always(QuotientIntegerCpuConstant)?,
                        Box::new(TwoArguments::QuadraticInXAndY(TwoArgumentsQuadraticFunction {
                            minimum: always(QuotientIntegerCpuMinimum)?,
                            coeff_00: always(QuotientIntegerCpuC00)?,
                            coeff_10: always(QuotientIntegerCpuC10)?,
                            coeff_01: always(QuotientIntegerCpuC01)?,
                            coeff_20: always(QuotientIntegerCpuC20)?,
                            coeff_11: always(QuotientIntegerCpuC11)?,
                            coeff_02: always(QuotientIntegerCpuC02)?,
                        })),
                    ),
                },
            },
            remainder_integer: Costing {
                mem: match semantics {
                    Semantics::A | Semantics::B => TwoArguments::SubtractedSizes(SubtractedSizes {
                        intercept: always(RemainderIntegerMemIntercept)?,
                        minimum: always(RemainderIntegerMemMinimum)?,
                        slope: always(RemainderIntegerMemSlope)?,
                    }),
                    Semantics::C | Semantics::D | Semantics::E => TwoArguments::LinearInY(LinearSize {
                        intercept: always(RemainderIntegerMemIntercept)?,
                        slope: always(RemainderIntegerMemSlope)?,
                    }),
                },
                cpu: match semantics {
                    Semantics::A | Semantics::B | Semantics::D => TwoArguments::ConstAboveDiagonal(
                        always(RemainderIntegerCpuConstant)?,
                        Box::new(TwoArguments::MultipliedSizes(MultipliedSizes {
                            intercept: always(RemainderIntegerCpuIntercept)?,
                            slope: always(RemainderIntegerCpuSlope)?,
                        })),
                    ),
                    Semantics::C | Semantics::E => TwoArguments::ConstAboveDiagonal(
                        always(RemainderIntegerCpuConstant)?,
                        Box::new(TwoArguments::QuadraticInXAndY(TwoArgumentsQuadraticFunction {
                            minimum: always(RemainderIntegerCpuMinimum)?,
                            coeff_00: always(RemainderIntegerCpuC00)?,
                            coeff_10: always(RemainderIntegerCpuC10)?,
                            coeff_01: always(RemainderIntegerCpuC01)?,
                            coeff_20: always(RemainderIntegerCpuC20)?,
                            coeff_11: always(RemainderIntegerCpuC11)?,
                            coeff_02: always(RemainderIntegerCpuC02)?,
                        })),
                    ),
                },
            },
            serialise_data: Costing {
                mem: OneArgument::LinearInX(LinearSize {
                    intercept: always(SerialiseDataMemIntercept)?,
                    slope: always(SerialiseDataMemSlope)?,
                }),
                cpu: OneArgument::LinearInX(LinearSize {
                    intercept: always(SerialiseDataCpuIntercept)?,
                    slope: always(SerialiseDataCpuSlope)?,
                }),
            },
            sha2_256: Costing {
                mem: OneArgument::Constant(always(Sha2256Mem)?),
                cpu: OneArgument::LinearInX(LinearSize {
                    intercept: always(Sha2256CpuIntercept)?,
                    slope: always(Sha2256CpuSlope)?,
                }),
            },
            sha3_256: Costing {
                mem: OneArgument::Constant(always(Sha3256Mem)?),
                cpu: OneArgument::LinearInX(LinearSize {
                    intercept: always(Sha3256CpuIntercept)?,
                    slope: always(Sha3256CpuSlope)?,
                }),
            },
            slice_byte_string: Costing {
                mem: ThreeArguments::LinearInZ(LinearSize {
                    intercept: always(SliceByteStringMemIntercept)?,
                    slope: always(SliceByteStringMemSlope)?,
                }),
                cpu: ThreeArguments::LinearInZ(LinearSize {
                    intercept: always(SliceByteStringCpuIntercept)?,
                    slope: always(SliceByteStringCpuSlope)?,
                }),
            },
            snd_pair: Costing {
                mem: OneArgument::Constant(always(SndPairMem)?),
                cpu: OneArgument::Constant(always(SndPairCpu)?),
            },
            subtract_integer: Costing {
                mem: TwoArguments::MaxSize(MaxSize {
                    intercept: always(SubtractIntegerMemIntercept)?,
                    slope: always(SubtractIntegerMemSlope)?,
                }),
                cpu: TwoArguments::MaxSize(MaxSize {
                    intercept: always(SubtractIntegerCpuIntercept)?,
                    slope: always(SubtractIntegerCpuSlope)?,
                }),
            },
            tail_list: Costing {
                mem: OneArgument::Constant(always(TailListMem)?),
                cpu: OneArgument::Constant(always(TailListCpu)?),
            },
            trace: Costing {
                mem: TwoArguments::Constant(always(TraceMem)?),
                cpu: TwoArguments::Constant(always(TraceCpu)?),
            },
            un_b_data: Costing {
                mem: OneArgument::Constant(always(UnBDataMem)?),
                cpu: OneArgument::Constant(always(UnBDataCpu)?),
            },
            un_constr_data: Costing {
                mem: OneArgument::Constant(always(UnConstrDataMem)?),
                cpu: OneArgument::Constant(always(UnConstrDataCpu)?),
            },
            un_i_data: Costing {
                mem: OneArgument::Constant(always(UnIDataMem)?),
                cpu: OneArgument::Constant(always(UnIDataCpu)?),
            },
            un_list_data: Costing {
                mem: OneArgument::Constant(always(UnListDataMem)?),
                cpu: OneArgument::Constant(always(UnListDataCpu)?),
            },
            un_map_data: Costing {
                mem: OneArgument::Constant(always(UnMapDataMem)?),
                cpu: OneArgument::Constant(always(UnMapDataCpu)?),
            },
            verify_ecdsa_secp256k1_signature: Costing {
                mem: ThreeArguments::Constant(always(VerifyEcdsaSecp256k1SignatureMem)?),
                cpu: ThreeArguments::Constant(always(VerifyEcdsaSecp256k1SignatureCpu)?),
            },
            verify_ed25519_signature: Costing {
                mem: ThreeArguments::Constant(always(VerifyEd25519SignatureMem)?),
                cpu: match semantics {
                    Semantics::A => ThreeArguments::LinearInZ(LinearSize {
                        intercept: always(VerifyEd25519SignatureCpuIntercept)?,
                        slope: always(VerifyEd25519SignatureCpuSlope)?,
                    }),
                    Semantics::B | Semantics::C | Semantics::D | Semantics::E => {
                        ThreeArguments::LinearInY(LinearSize {
                            intercept: always(VerifyEd25519SignatureCpuIntercept)?,
                            slope: always(VerifyEd25519SignatureCpuSlope)?,
                        })
                    }
                },
            },
            verify_schnorr_secp256k1_signature: Costing {
                mem: ThreeArguments::Constant(always(VerifySchnorrSecp256k1SignatureMem)?),
                cpu: ThreeArguments::LinearInY(LinearSize {
                    intercept: always(VerifySchnorrSecp256k1SignatureCpuIntercept)?,
                    slope: always(VerifySchnorrSecp256k1SignatureCpuSlope)?,
                }),
            },
            bls12_381_g1_add: Costing {
                cpu: TwoArguments::Constant(always(BlsG1AddCpu)?),
                mem: TwoArguments::Constant(always(BlsG1AddMem)?),
            },
            bls12_381_g1_compress: Costing {
                cpu: OneArgument::Constant(always(BlsG1CompressCpu)?),
                mem: OneArgument::Constant(always(BlsG1CompressMem)?),
            },
            bls12_381_g1_equal: Costing {
                cpu: TwoArguments::Constant(always(BlsG1EqualCpu)?),
                mem: TwoArguments::Constant(always(BlsG1EqualMem)?),
            },
            bls12_381_g1_hash_to_group: Costing {
                cpu: TwoArguments::LinearInX(LinearSize {
                    intercept: always(BlsG1HashToGroupCpuIntercept)?,
                    slope: always(BlsG1HashToGroupCpuSlope)?,
                }),
                mem: TwoArguments::Constant(always(BlsG1HashToGroupMem)?),
            },
            bls12_381_g1_neg: Costing {
                cpu: OneArgument::Constant(always(BlsG1NegCpu)?),
                mem: OneArgument::Constant(always(BlsG1NegMem)?),
            },
            bls12_381_g1_scalar_mul: Costing {
                cpu: TwoArguments::LinearInX(LinearSize {
                    intercept: always(BlsG1ScalarMulCpuIntercept)?,
                    slope: always(BlsG1ScalarMulCpuSlope)?,
                }),
                mem: TwoArguments::Constant(always(BlsG1ScalarMulMem)?),
            },
            bls12_381_g1_uncompress: Costing {
                cpu: OneArgument::Constant(always(BlsG1UncompressCpu)?),
                mem: OneArgument::Constant(always(BlsG1UncompressMem)?),
            },
            bls12_381_g2_add: Costing {
                cpu: TwoArguments::Constant(always(BlsG2AddCpu)?),
                mem: TwoArguments::Constant(always(BlsG2AddMem)?),
            },
            bls12_381_g2_compress: Costing {
                cpu: OneArgument::Constant(always(BlsG2CompressCpu)?),
                mem: OneArgument::Constant(always(BlsG2CompressMem)?),
            },
            bls12_381_g2_equal: Costing {
                cpu: TwoArguments::Constant(always(BlsG2EqualCpu)?),
                mem: TwoArguments::Constant(always(BlsG2EqualMem)?),
            },
            bls12_381_g2_hash_to_group: Costing {
                cpu: TwoArguments::LinearInX(LinearSize {
                    intercept: always(BlsG2HashToGroupCpuIntercept)?,
                    slope: always(BlsG2HashToGroupCpuSlope)?,
                }),
                mem: TwoArguments::Constant(always(BlsG2HashToGroupMem)?),
            },
            bls12_381_g2_neg: Costing {
                cpu: OneArgument::Constant(always(BlsG2NegCpu)?),
                mem: OneArgument::Constant(always(BlsG2NegMem)?),
            },
            bls12_381_g2_scalar_mul: Costing {
                cpu: TwoArguments::LinearInX(LinearSize {
                    intercept: always(BlsG2ScalarMulCpuIntercept)?,
                    slope: always(BlsG2ScalarMulCpuSlope)?,
                }),
                mem: TwoArguments::Constant(always(BlsG2ScalarMulMem)?),
            },
            bls12_381_g2_uncompress: Costing {
                cpu: OneArgument::Constant(always(BlsG2UncompressCpu)?),
                mem: OneArgument::Constant(always(BlsG2UncompressMem)?),
            },
            bls12_381_final_verify: Costing {
                cpu: TwoArguments::Constant(always(BlsFinalVerifyCpu)?),
                mem: TwoArguments::Constant(always(BlsFinalVerifyMem)?),
            },
            bls12_381_miller_loop: Costing {
                cpu: TwoArguments::Constant(always(BlsMillerLoopCpu)?),
                mem: TwoArguments::Constant(always(BlsMillerLoopMem)?),
            },
            bls12_381_mul_ml_result: Costing {
                cpu: TwoArguments::Constant(always(BlsMulMlResultCpu)?),
                mem: TwoArguments::Constant(always(BlsMulMlResultMem)?),
            },
            keccak_256: Costing {
                cpu: OneArgument::LinearInX(LinearSize {
                    intercept: always(Keccak256CpuIntercept)?,
                    slope: always(Keccak256CpuSlope)?,
                }),
                mem: OneArgument::Constant(always(Keccak256Mem)?),
            },
            blake2b_224: Costing {
                cpu: OneArgument::LinearInX(LinearSize {
                    intercept: always(Blake2b224CpuIntercept)?,
                    slope: always(Blake2b224CpuSlope)?,
                }),
                mem: OneArgument::Constant(always(Blake2b224Mem)?),
            },
            integer_to_byte_string: Costing {
                cpu: ThreeArguments::QuadraticInZ(QuadraticFunction {
                    coeff_0: always(IntegerToByteStringCpuC0)?,
                    coeff_1: always(IntegerToByteStringCpuC1)?,
                    coeff_2: always(IntegerToByteStringCpuC2)?,
                }),
                mem: ThreeArguments::LiteralInYorLinearInZ(LinearSize {
                    intercept: always(IntegerToByteStringMemIntercept)?,
                    slope: always(IntegerToByteStringMemSlope)?,
                }),
            },
            byte_string_to_integer: Costing {
                cpu: TwoArguments::QuadraticInY(QuadraticFunction {
                    coeff_0: always(ByteStringToIntegerCpuC0)?,
                    coeff_1: always(ByteStringToIntegerCpuC1)?,
                    coeff_2: always(ByteStringToIntegerCpuC2)?,
                }),
                mem: TwoArguments::LinearInY(LinearSize {
                    intercept: always(ByteStringToIntegerMemIntercept)?,
                    slope: always(ByteStringToIntegerMemSlope)?,
                }),
            },

            // Starting from ProtocolVersion >= 10
            and_byte_string: Costing {
                cpu: ThreeArguments::LinearInYAndZ(TwoVariableLinearSize {
                    intercept: if_pv10(AndByteStringCpuIntercept)?,
                    slope1: if_pv10(AndByteStringCpuSlope1)?,
                    slope2: if_pv10(AndByteStringCpuSlope2)?,
                }),
                mem: ThreeArguments::LinearInMaxYZ(LinearSize {
                    intercept: if_pv10(AndByteStringMemIntercept)?,
                    slope: if_pv10(AndByteStringMemSlope)?,
                }),
            },
            or_byte_string: Costing {
                cpu: ThreeArguments::LinearInYAndZ(TwoVariableLinearSize {
                    intercept: if_pv10(OrByteStringCpuIntercept)?,
                    slope1: if_pv10(OrByteStringCpuSlope1)?,
                    slope2: if_pv10(OrByteStringCpuSlope2)?,
                }),
                mem: ThreeArguments::LinearInMaxYZ(LinearSize {
                    intercept: if_pv10(OrByteStringMemIntercept)?,
                    slope: if_pv10(OrByteStringMemSlope)?,
                }),
            },
            xor_byte_string: Costing {
                cpu: ThreeArguments::LinearInYAndZ(TwoVariableLinearSize {
                    intercept: if_pv10(XorByteStringCpuIntercept)?,
                    slope1: if_pv10(XorByteStringCpuSlope1)?,
                    slope2: if_pv10(XorByteStringCpuSlope2)?,
                }),
                mem: ThreeArguments::LinearInMaxYZ(LinearSize {
                    intercept: if_pv10(XorByteStringMemIntercept)?,
                    slope: if_pv10(XorByteStringMemSlope)?,
                }),
            },
            complement_byte_string: Costing {
                cpu: OneArgument::LinearInX(LinearSize {
                    intercept: if_pv10(ComplementByteStringCpuIntercept)?,
                    slope: if_pv10(ComplementByteStringCpuSlope)?,
                }),
                mem: OneArgument::LinearInX(LinearSize {
                    intercept: if_pv10(ComplementByteStringMemIntercept)?,
                    slope: if_pv10(ComplementByteStringMemSlope)?,
                }),
            },
            read_bit: Costing {
                cpu: TwoArguments::Constant(if_pv10(ReadBitCpu)?),
                mem: TwoArguments::Constant(if_pv10(ReadBitMem)?),
            },
            write_bits: Costing {
                cpu: ThreeArguments::LinearInY(LinearSize {
                    intercept: if_pv10(WriteBitsCpuIntercept)?,
                    slope: if_pv10(WriteBitsCpuSlope)?,
                }),
                mem: ThreeArguments::LinearInX(LinearSize {
                    intercept: if_pv10(WriteBitsMemIntercept)?,
                    slope: if_pv10(WriteBitsMemSlope)?,
                }),
            },
            replicate_byte: Costing {
                cpu: TwoArguments::LinearInX(LinearSize {
                    intercept: if_pv10(ReplicateByteCpuIntercept)?,
                    slope: if_pv10(ReplicateByteCpuSlope)?,
                }),
                mem: TwoArguments::LinearInX(LinearSize {
                    intercept: if_pv10(ReplicateByteMemIntercept)?,
                    slope: if_pv10(ReplicateByteMemSlope)?,
                }),
            },
            shift_byte_string: Costing {
                cpu: TwoArguments::LinearInX(LinearSize {
                    intercept: if_pv10(ShiftByteStringCpuIntercept)?,
                    slope: if_pv10(ShiftByteStringCpuSlope)?,
                }),
                mem: TwoArguments::LinearInX(LinearSize {
                    intercept: if_pv10(ShiftByteStringMemIntercept)?,
                    slope: if_pv10(ShiftByteStringMemSlope)?,
                }),
            },
            rotate_byte_string: Costing {
                cpu: TwoArguments::LinearInX(LinearSize {
                    intercept: if_pv10(RotateByteStringCpuIntercept)?,
                    slope: if_pv10(RotateByteStringCpuSlope)?,
                }),
                mem: TwoArguments::LinearInX(LinearSize {
                    intercept: if_pv10(RotateByteStringMemIntercept)?,
                    slope: if_pv10(RotateByteStringMemSlope)?,
                }),
            },
            count_set_bits: Costing {
                cpu: OneArgument::LinearInX(LinearSize {
                    intercept: if_pv10(CountSetBitsCpuIntercept)?,
                    slope: if_pv10(CountSetBitsCpuSlope)?,
                }),
                mem: OneArgument::Constant(if_pv10(CountSetBitsMem)?),
            },
            find_first_set_bit: Costing {
                cpu: OneArgument::LinearInX(LinearSize {
                    intercept: if_pv10(FindFirstSetBitCpuIntercept)?,
                    slope: if_pv10(FindFirstSetBitCpuSlope)?,
                }),
                mem: OneArgument::Constant(if_pv10(FindFirstSetBitMem)?),
            },
            ripemd_160: Costing {
                cpu: OneArgument::LinearInX(LinearSize {
                    intercept: if_pv10(Ripemd160CpuIntercept)?,
                    slope: if_pv10(Ripemd160CpuSlope)?,
                }),
                mem: OneArgument::Constant(if_pv10(Ripemd160Mem)?),
            },

            // Starting from ProtocolVersion >= 11
            exp_mod_integer: Costing {
                cpu: ThreeArguments::ExpModCost(ExpModCost {
                    coeff_00: if_pv11(ExpModIntegerCpuC00)?,
                    coeff_11: if_pv11(ExpModIntegerCpuC11)?,
                    coeff_12: if_pv11(ExpModIntegerCpuC12)?,
                }),
                mem: ThreeArguments::LinearInZ(LinearSize {
                    intercept: if_pv11(ExpModIntegerMemIntercept)?,
                    slope: if_pv11(ExpModIntegerMemSlope)?,
                }),
            },
            drop_list: Costing {
                cpu: TwoArguments::LinearInX(LinearSize {
                    intercept: if_pv11(DropListCpuIntercept)?,
                    slope: if_pv11(DropListCpuSlope)?,
                }),
                mem: TwoArguments::Constant(if_pv11(DropListMem)?),
            },
            length_of_array: Costing {
                cpu: OneArgument::Constant(if_pv11(LengthOfArrayCpu)?),
                mem: OneArgument::Constant(if_pv11(LengthOfArrayMem)?),
            },
            list_to_array: Costing {
                cpu: OneArgument::LinearInX(LinearSize {
                    intercept: if_pv11(ListToArrayCpuIntercept)?,
                    slope: if_pv11(ListToArrayCpuSlope)?,
                }),
                mem: OneArgument::LinearInX(LinearSize {
                    intercept: if_pv11(ListToArrayMemIntercept)?,
                    slope: if_pv11(ListToArrayMemSlope)?,
                }),
            },
            index_array: Costing {
                cpu: TwoArguments::Constant(if_pv11(IndexArrayCpu)?),
                mem: TwoArguments::Constant(if_pv11(IndexArrayMem)?),
            },
            bls12_381_g1_multi_scalar_mul: Costing {
                cpu: TwoArguments::LinearInX(LinearSize {
                    intercept: if_pv11(BlsG1MultiScalarMulCpuIntercept)?,
                    slope: if_pv11(BlsG1MultiScalarMulCpuSlope)?,
                }),
                mem: TwoArguments::Constant(if_pv11(BlsG1MultiScalarMulMem)?),
            },
            bls12_381_g2_multi_scalar_mul: Costing {
                cpu: TwoArguments::LinearInX(LinearSize {
                    intercept: if_pv11(BlsG2MultiScalarMulCpuIntercept)?,
                    slope: if_pv11(BlsG2MultiScalarMulCpuSlope)?,
                }),
                mem: TwoArguments::Constant(if_pv11(BlsG2MultiScalarMulMem)?),
            },
            insert_coin: Costing {
                cpu: FourArguments::LinearInU(LinearSize {
                    intercept: if_pv11(InsertCoinCpuIntercept)?,
                    slope: if_pv11(InsertCoinCpuSlope)?,
                }),
                mem: FourArguments::LinearInU(LinearSize {
                    intercept: if_pv11(InsertCoinMemIntercept)?,
                    slope: if_pv11(InsertCoinMemSlope)?,
                }),
            },
            lookup_coin: Costing {
                cpu: ThreeArguments::LinearInZ(LinearSize {
                    intercept: if_pv11(LookupCoinCpuIntercept)?,
                    slope: if_pv11(LookupCoinCpuSlope)?,
                }),
                mem: ThreeArguments::Constant(if_pv11(LookupCoinMem)?),
            },
            union_value: Costing {
                cpu: TwoArguments::WithInteraction(WithInteraction {
                    coeff_00: if_pv11(UnionValueCpuC00)?,
                    coeff_10: if_pv11(UnionValueCpuC10)?,
                    coeff_01: if_pv11(UnionValueCpuC01)?,
                    coeff_11: if_pv11(UnionValueCpuC11)?,
                }),
                mem: TwoArguments::AddedSizes(AddedSizes {
                    intercept: if_pv11(UnionValueMemIntercept)?,
                    slope: if_pv11(UnionValueMemSlope)?,
                }),
            },
            value_contains: Costing {
                cpu: TwoArguments::ConstAboveDiagonal(
                    if_pv11(ValueContainsCpuConstant)?,
                    Box::new(TwoArguments::LinearInXAndY(TwoVariableLinearSize {
                        intercept: if_pv11(ValueContainsCpuIntercept)?,
                        slope1: if_pv11(ValueContainsCpuSlope1)?,
                        slope2: if_pv11(ValueContainsCpuSlope2)?,
                    })),
                ),
                mem: TwoArguments::Constant(if_pv11(ValueContainsMem)?),
            },
            value_data: Costing {
                cpu: OneArgument::LinearInX(LinearSize {
                    intercept: if_pv11(ValueDataCpuIntercept)?,
                    slope: if_pv11(ValueDataCpuSlope)?,
                }),
                mem: OneArgument::LinearInX(LinearSize {
                    intercept: if_pv11(ValueDataMemIntercept)?,
                    slope: if_pv11(ValueDataMemSlope)?,
                }),
            },
            un_value_data: Costing {
                cpu: OneArgument::Quadratic(QuadraticFunction {
                    coeff_0: if_pv11(UnValueDataCpuC0)?,
                    coeff_1: if_pv11(UnValueDataCpuC1)?,
                    coeff_2: if_pv11(UnValueDataCpuC2)?,
                }),
                mem: OneArgument::LinearInX(LinearSize {
                    intercept: if_pv11(UnValueDataMemIntercept)?,
                    slope: if_pv11(UnValueDataMemSlope)?,
                }),
            },
            scale_value: Costing {
                cpu: TwoArguments::LinearInY(LinearSize {
                    intercept: if_pv11(ScaleValueCpuIntercept)?,
                    slope: if_pv11(ScaleValueCpuSlope)?,
                }),
                mem: TwoArguments::LinearInY(LinearSize {
                    intercept: if_pv11(ScaleValueMemIntercept)?,
                    slope: if_pv11(ScaleValueMemSlope)?,
                }),
            },
        })
    }

    pub fn get_cost(&self, builtin: DefaultFunction, args: &[i64]) -> Option<ExBudget> {
        match builtin {
            DefaultFunction::AddInteger => Some(ExBudget::new(
                self.add_integer.mem.cost([args[0], args[1]]),
                self.add_integer.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::SubtractInteger => Some(ExBudget::new(
                self.subtract_integer.mem.cost([args[0], args[1]]),
                self.subtract_integer.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::MultiplyInteger => Some(ExBudget::new(
                self.multiply_integer.mem.cost([args[0], args[1]]),
                self.multiply_integer.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::DivideInteger => Some(ExBudget::new(
                self.divide_integer.mem.cost([args[0], args[1]]),
                self.divide_integer.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::QuotientInteger => Some(ExBudget::new(
                self.quotient_integer.mem.cost([args[0], args[1]]),
                self.quotient_integer.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::RemainderInteger => Some(ExBudget::new(
                self.remainder_integer.mem.cost([args[0], args[1]]),
                self.remainder_integer.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::ModInteger => Some(ExBudget::new(
                self.mod_integer.mem.cost([args[0], args[1]]),
                self.mod_integer.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::EqualsInteger => Some(ExBudget::new(
                self.equals_integer.mem.cost([args[0], args[1]]),
                self.equals_integer.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::LessThanInteger => Some(ExBudget::new(
                self.less_than_integer.mem.cost([args[0], args[1]]),
                self.less_than_integer.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::LessThanEqualsInteger => Some(ExBudget::new(
                self.less_than_equals_integer.mem.cost([args[0], args[1]]),
                self.less_than_equals_integer.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::AppendByteString => Some(ExBudget::new(
                self.append_byte_string.mem.cost([args[0], args[1]]),
                self.append_byte_string.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::ConsByteString => Some(ExBudget::new(
                self.cons_byte_string.mem.cost([args[0], args[1]]),
                self.cons_byte_string.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::SliceByteString => Some(ExBudget::new(
                self.slice_byte_string.mem.cost([args[0], args[1], args[2]]),
                self.slice_byte_string.cpu.cost([args[0], args[1], args[2]]),
            )),
            DefaultFunction::LengthOfByteString => Some(ExBudget::new(
                self.length_of_byte_string.mem.cost([args[0]]),
                self.length_of_byte_string.cpu.cost([args[0]]),
            )),
            DefaultFunction::IndexByteString => Some(ExBudget::new(
                self.index_byte_string.mem.cost([args[0], args[1]]),
                self.index_byte_string.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::EqualsByteString => Some(ExBudget::new(
                self.equals_byte_string.mem.cost([args[0], args[1]]),
                self.equals_byte_string.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::LessThanByteString => Some(ExBudget::new(
                self.less_than_byte_string.mem.cost([args[0], args[1]]),
                self.less_than_byte_string.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::LessThanEqualsByteString => Some(ExBudget::new(
                self.less_than_equals_byte_string.mem.cost([args[0], args[1]]),
                self.less_than_equals_byte_string.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::Sha2_256 => {
                Some(ExBudget::new(self.sha2_256.mem.cost([args[0]]), self.sha2_256.cpu.cost([args[0]])))
            }
            DefaultFunction::Sha3_256 => {
                Some(ExBudget::new(self.sha3_256.mem.cost([args[0]]), self.sha3_256.cpu.cost([args[0]])))
            }
            DefaultFunction::Blake2b_224 => {
                Some(ExBudget::new(self.blake2b_224.mem.cost([args[0]]), self.blake2b_224.cpu.cost([args[0]])))
            }
            DefaultFunction::Blake2b_256 => {
                Some(ExBudget::new(self.blake2b_256.mem.cost([args[0]]), self.blake2b_256.cpu.cost([args[0]])))
            }
            DefaultFunction::Keccak_256 => {
                Some(ExBudget::new(self.keccak_256.mem.cost([args[0]]), self.keccak_256.cpu.cost([args[0]])))
            }
            DefaultFunction::VerifyEd25519Signature => Some(ExBudget::new(
                self.verify_ed25519_signature.mem.cost([args[0], args[1], args[2]]),
                self.verify_ed25519_signature.cpu.cost([args[0], args[1], args[2]]),
            )),
            DefaultFunction::VerifyEcdsaSecp256k1Signature => Some(ExBudget::new(
                self.verify_ecdsa_secp256k1_signature.mem.cost([args[0], args[1], args[2]]),
                self.verify_ecdsa_secp256k1_signature.cpu.cost([args[0], args[1], args[2]]),
            )),
            DefaultFunction::VerifySchnorrSecp256k1Signature => Some(ExBudget::new(
                self.verify_schnorr_secp256k1_signature.mem.cost([args[0], args[1], args[2]]),
                self.verify_schnorr_secp256k1_signature.cpu.cost([args[0], args[1], args[2]]),
            )),
            DefaultFunction::AppendString => Some(ExBudget::new(
                self.append_string.mem.cost([args[0], args[1]]),
                self.append_string.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::EqualsString => Some(ExBudget::new(
                self.equals_string.mem.cost([args[0], args[1]]),
                self.equals_string.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::EncodeUtf8 => {
                Some(ExBudget::new(self.encode_utf8.mem.cost([args[0]]), self.encode_utf8.cpu.cost([args[0]])))
            }
            DefaultFunction::DecodeUtf8 => {
                Some(ExBudget::new(self.decode_utf8.mem.cost([args[0]]), self.decode_utf8.cpu.cost([args[0]])))
            }
            DefaultFunction::IfThenElse => Some(ExBudget::new(
                self.if_then_else.mem.cost([args[0], args[1], args[2]]),
                self.if_then_else.cpu.cost([args[0], args[1], args[2]]),
            )),
            DefaultFunction::ChooseUnit => Some(ExBudget::new(
                self.choose_unit.mem.cost([args[0], args[1]]),
                self.choose_unit.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::Trace => {
                Some(ExBudget::new(self.trace.mem.cost([args[0], args[1]]), self.trace.cpu.cost([args[0], args[1]])))
            }
            DefaultFunction::FstPair => {
                Some(ExBudget::new(self.fst_pair.mem.cost([args[0]]), self.fst_pair.cpu.cost([args[0]])))
            }
            DefaultFunction::SndPair => {
                Some(ExBudget::new(self.snd_pair.mem.cost([args[0]]), self.snd_pair.cpu.cost([args[0]])))
            }
            DefaultFunction::ChooseList => Some(ExBudget::new(
                self.choose_list.mem.cost([args[0], args[1], args[2]]),
                self.choose_list.cpu.cost([args[0], args[1], args[2]]),
            )),
            DefaultFunction::MkCons => Some(ExBudget::new(
                self.mk_cons.mem.cost([args[0], args[1]]),
                self.mk_cons.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::HeadList => {
                Some(ExBudget::new(self.head_list.mem.cost([args[0]]), self.head_list.cpu.cost([args[0]])))
            }
            DefaultFunction::TailList => {
                Some(ExBudget::new(self.tail_list.mem.cost([args[0]]), self.tail_list.cpu.cost([args[0]])))
            }
            DefaultFunction::NullList => {
                Some(ExBudget::new(self.null_list.mem.cost([args[0]]), self.null_list.cpu.cost([args[0]])))
            }
            DefaultFunction::ChooseData => Some(ExBudget::new(
                self.choose_data.mem.cost([args[0], args[1], args[2], args[3], args[4], args[5]]),
                self.choose_data.cpu.cost([args[0], args[1], args[2], args[3], args[4], args[5]]),
            )),
            DefaultFunction::ConstrData => Some(ExBudget::new(
                self.constr_data.mem.cost([args[0], args[1]]),
                self.constr_data.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::MapData => {
                Some(ExBudget::new(self.map_data.mem.cost([args[0]]), self.map_data.cpu.cost([args[0]])))
            }
            DefaultFunction::ListData => {
                Some(ExBudget::new(self.list_data.mem.cost([args[0]]), self.list_data.cpu.cost([args[0]])))
            }
            DefaultFunction::IData => {
                Some(ExBudget::new(self.i_data.mem.cost([args[0]]), self.i_data.cpu.cost([args[0]])))
            }
            DefaultFunction::BData => {
                Some(ExBudget::new(self.b_data.mem.cost([args[0]]), self.b_data.cpu.cost([args[0]])))
            }
            DefaultFunction::UnConstrData => {
                Some(ExBudget::new(self.un_constr_data.mem.cost([args[0]]), self.un_constr_data.cpu.cost([args[0]])))
            }
            DefaultFunction::UnMapData => {
                Some(ExBudget::new(self.un_map_data.mem.cost([args[0]]), self.un_map_data.cpu.cost([args[0]])))
            }
            DefaultFunction::UnListData => {
                Some(ExBudget::new(self.un_list_data.mem.cost([args[0]]), self.un_list_data.cpu.cost([args[0]])))
            }
            DefaultFunction::UnIData => {
                Some(ExBudget::new(self.un_i_data.mem.cost([args[0]]), self.un_i_data.cpu.cost([args[0]])))
            }
            DefaultFunction::UnBData => {
                Some(ExBudget::new(self.un_b_data.mem.cost([args[0]]), self.un_b_data.cpu.cost([args[0]])))
            }
            DefaultFunction::EqualsData => Some(ExBudget::new(
                self.equals_data.mem.cost([args[0], args[1]]),
                self.equals_data.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::MkPairData => Some(ExBudget::new(
                self.mk_pair_data.mem.cost([args[0], args[1]]),
                self.mk_pair_data.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::MkNilData => {
                Some(ExBudget::new(self.mk_nil_data.mem.cost([args[0]]), self.mk_nil_data.cpu.cost([args[0]])))
            }
            DefaultFunction::MkNilPairData => Some(ExBudget::new(
                self.mk_nil_pair_data.mem.cost([args[0]]),
                self.mk_nil_pair_data.cpu.cost([args[0]]),
            )),
            DefaultFunction::SerialiseData => {
                Some(ExBudget::new(self.serialise_data.mem.cost([args[0]]), self.serialise_data.cpu.cost([args[0]])))
            }
            DefaultFunction::Bls12_381_G1_Add => Some(ExBudget::new(
                self.bls12_381_g1_add.mem.cost([args[0], args[1]]),
                self.bls12_381_g1_add.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::Bls12_381_G1_Neg => Some(ExBudget::new(
                self.bls12_381_g1_neg.mem.cost([args[0]]),
                self.bls12_381_g1_neg.cpu.cost([args[0]]),
            )),
            DefaultFunction::Bls12_381_G1_ScalarMul => Some(ExBudget::new(
                self.bls12_381_g1_scalar_mul.mem.cost([args[0], args[1]]),
                self.bls12_381_g1_scalar_mul.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::Bls12_381_G1_Equal => Some(ExBudget::new(
                self.bls12_381_g1_equal.mem.cost([args[0], args[1]]),
                self.bls12_381_g1_equal.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::Bls12_381_G1_Compress => Some(ExBudget::new(
                self.bls12_381_g1_compress.mem.cost([args[0]]),
                self.bls12_381_g1_compress.cpu.cost([args[0]]),
            )),
            DefaultFunction::Bls12_381_G1_Uncompress => Some(ExBudget::new(
                self.bls12_381_g1_uncompress.mem.cost([args[0]]),
                self.bls12_381_g1_uncompress.cpu.cost([args[0]]),
            )),
            DefaultFunction::Bls12_381_G1_HashToGroup => Some(ExBudget::new(
                self.bls12_381_g1_hash_to_group.mem.cost([args[0], args[1]]),
                self.bls12_381_g1_hash_to_group.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::Bls12_381_G2_Add => Some(ExBudget::new(
                self.bls12_381_g2_add.mem.cost([args[0], args[1]]),
                self.bls12_381_g2_add.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::Bls12_381_G2_Neg => Some(ExBudget::new(
                self.bls12_381_g2_neg.mem.cost([args[0]]),
                self.bls12_381_g2_neg.cpu.cost([args[0]]),
            )),
            DefaultFunction::Bls12_381_G2_ScalarMul => Some(ExBudget::new(
                self.bls12_381_g2_scalar_mul.mem.cost([args[0], args[1]]),
                self.bls12_381_g2_scalar_mul.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::Bls12_381_G2_Equal => Some(ExBudget::new(
                self.bls12_381_g2_equal.mem.cost([args[0], args[1]]),
                self.bls12_381_g2_equal.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::Bls12_381_G2_Compress => Some(ExBudget::new(
                self.bls12_381_g2_compress.mem.cost([args[0]]),
                self.bls12_381_g2_compress.cpu.cost([args[0]]),
            )),
            DefaultFunction::Bls12_381_G2_Uncompress => Some(ExBudget::new(
                self.bls12_381_g2_uncompress.mem.cost([args[0]]),
                self.bls12_381_g2_uncompress.cpu.cost([args[0]]),
            )),
            DefaultFunction::Bls12_381_G2_HashToGroup => Some(ExBudget::new(
                self.bls12_381_g2_hash_to_group.mem.cost([args[0], args[1]]),
                self.bls12_381_g2_hash_to_group.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::Bls12_381_MillerLoop => Some(ExBudget::new(
                self.bls12_381_miller_loop.mem.cost([args[0], args[1]]),
                self.bls12_381_miller_loop.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::Bls12_381_MulMlResult => Some(ExBudget::new(
                self.bls12_381_mul_ml_result.mem.cost([args[0], args[1]]),
                self.bls12_381_mul_ml_result.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::Bls12_381_FinalVerify => Some(ExBudget::new(
                self.bls12_381_final_verify.mem.cost([args[0], args[1]]),
                self.bls12_381_final_verify.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::IntegerToByteString => Some(ExBudget::new(
                self.integer_to_byte_string.mem.cost([args[0], args[1], args[2]]),
                self.integer_to_byte_string.cpu.cost([args[0], args[1], args[2]]),
            )),
            DefaultFunction::ByteStringToInteger => Some(ExBudget::new(
                self.byte_string_to_integer.mem.cost([args[0], args[1]]),
                self.byte_string_to_integer.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::AndByteString => Some(ExBudget::new(
                self.and_byte_string.mem.cost([args[0], args[1], args[2]]),
                self.and_byte_string.cpu.cost([args[0], args[1], args[2]]),
            )),
            DefaultFunction::OrByteString => Some(ExBudget::new(
                self.or_byte_string.mem.cost([args[0], args[1], args[2]]),
                self.or_byte_string.cpu.cost([args[0], args[1], args[2]]),
            )),
            DefaultFunction::XorByteString => Some(ExBudget::new(
                self.xor_byte_string.mem.cost([args[0], args[1], args[2]]),
                self.xor_byte_string.cpu.cost([args[0], args[1], args[2]]),
            )),
            DefaultFunction::ComplementByteString => Some(ExBudget::new(
                self.complement_byte_string.mem.cost([args[0]]),
                self.complement_byte_string.cpu.cost([args[0]]),
            )),
            DefaultFunction::ReadBit => Some(ExBudget::new(
                self.read_bit.mem.cost([args[0], args[1]]),
                self.read_bit.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::WriteBits => Some(ExBudget::new(
                self.write_bits.mem.cost([args[0], args[1], args[2]]),
                self.write_bits.cpu.cost([args[0], args[1], args[2]]),
            )),
            DefaultFunction::ReplicateByte => Some(ExBudget::new(
                self.replicate_byte.mem.cost([args[0], args[1]]),
                self.replicate_byte.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::ShiftByteString => Some(ExBudget::new(
                self.shift_byte_string.mem.cost([args[0], args[1]]),
                self.shift_byte_string.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::RotateByteString => Some(ExBudget::new(
                self.rotate_byte_string.mem.cost([args[0], args[1]]),
                self.rotate_byte_string.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::CountSetBits => {
                Some(ExBudget::new(self.count_set_bits.mem.cost([args[0]]), self.count_set_bits.cpu.cost([args[0]])))
            }
            DefaultFunction::FindFirstSetBit => Some(ExBudget::new(
                self.find_first_set_bit.mem.cost([args[0]]),
                self.find_first_set_bit.cpu.cost([args[0]]),
            )),
            DefaultFunction::Ripemd_160 => {
                Some(ExBudget::new(self.ripemd_160.mem.cost([args[0]]), self.ripemd_160.cpu.cost([args[0]])))
            }
            DefaultFunction::ExpModInteger => Some(ExBudget::new(
                self.exp_mod_integer.mem.cost([args[0], args[1], args[2]]),
                self.exp_mod_integer.cpu.cost([args[0], args[1], args[2]]),
            )),
            DefaultFunction::DropList => Some(ExBudget::new(
                self.drop_list.mem.cost([args[0], args[1]]),
                self.drop_list.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::LengthOfArray => {
                Some(ExBudget::new(self.length_of_array.mem.cost([args[0]]), self.length_of_array.cpu.cost([args[0]])))
            }
            DefaultFunction::ListToArray => {
                Some(ExBudget::new(self.list_to_array.mem.cost([args[0]]), self.list_to_array.cpu.cost([args[0]])))
            }
            DefaultFunction::IndexArray => Some(ExBudget::new(
                self.index_array.mem.cost([args[0], args[1]]),
                self.index_array.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::Bls12_381_G1_MultiScalarMul => Some(ExBudget::new(
                self.bls12_381_g1_multi_scalar_mul.mem.cost([args[0], args[1]]),
                self.bls12_381_g1_multi_scalar_mul.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::Bls12_381_G2_MultiScalarMul => Some(ExBudget::new(
                self.bls12_381_g2_multi_scalar_mul.mem.cost([args[0], args[1]]),
                self.bls12_381_g2_multi_scalar_mul.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::InsertCoin => Some(ExBudget::new(
                self.insert_coin.mem.cost([args[0], args[1], args[2], args[3]]),
                self.insert_coin.cpu.cost([args[0], args[1], args[2], args[3]]),
            )),
            DefaultFunction::LookupCoin => Some(ExBudget::new(
                self.lookup_coin.mem.cost([args[0], args[1], args[2]]),
                self.lookup_coin.cpu.cost([args[0], args[1], args[2]]),
            )),
            DefaultFunction::UnionValue => Some(ExBudget::new(
                self.union_value.mem.cost([args[0], args[1]]),
                self.union_value.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::ValueContains => Some(ExBudget::new(
                self.value_contains.mem.cost([args[0], args[1]]),
                self.value_contains.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::ValueData => {
                Some(ExBudget::new(self.value_data.mem.cost([args[0]]), self.value_data.cpu.cost([args[0]])))
            }
            DefaultFunction::UnValueData => {
                Some(ExBudget::new(self.un_value_data.mem.cost([args[0]]), self.un_value_data.cpu.cost([args[0]])))
            }
            DefaultFunction::ScaleValue => Some(ExBudget::new(
                self.scale_value.mem.cost([args[0], args[1]]),
                self.scale_value.cpu.cost([args[0], args[1]]),
            )),
        }
    }
}
