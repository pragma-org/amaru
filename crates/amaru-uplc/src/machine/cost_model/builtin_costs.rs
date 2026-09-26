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

use crate::{
    builtin::DefaultFunction,
    machine::{
        CostModel, ExBudget, Semantics,
        cost_model::{
            ParamName,
            cost_argument::CostArgument,
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

fn cost<const N: usize, T>(costing: &Costing<N, T>, args: &[CostArgument<'_>]) -> ExBudget
where
    T: Cost<N>,
{
    ExBudget::new(costing.mem.cost(args), costing.cpu.cost(args))
}

impl BuiltinCosts {
    #[allow(clippy::disallowed_types)]
    pub fn new(cost_map: &HashMap<ParamName, i64>, semantics: Semantics) -> Self {
        use ParamName::*;

        // NOTE: About missing cost models
        //
        // We must tolerate having less cost params in the map than the total we know of; this is
        // because builtins and parameters are typically introduced before new cost models are
        // enacted and available on-chain. The current default is to use the maximum possible
        // value, making the builtins practically unusable until the introduction of the cost
        // model.
        let param = |name: ParamName| cost_map.get(&name).copied().unwrap_or(i64::MAX);

        Self {
            add_integer: Costing {
                mem: TwoArguments::MaxSize(MaxSize {
                    intercept: param(AddIntegerMemIntercept),
                    slope: param(AddIntegerMemSlope),
                }),
                cpu: TwoArguments::MaxSize(MaxSize {
                    intercept: param(AddIntegerCpuIntercept),
                    slope: param(AddIntegerCpuSlope),
                }),
            },
            append_byte_string: Costing {
                mem: TwoArguments::AddedSizes(AddedSizes {
                    intercept: param(AppendByteStringMemIntercept),
                    slope: param(AppendByteStringMemSlope),
                }),
                cpu: TwoArguments::AddedSizes(AddedSizes {
                    intercept: param(AppendByteStringCpuIntercept),
                    slope: param(AppendByteStringCpuSlope),
                }),
            },
            append_string: Costing {
                mem: TwoArguments::AddedSizes(AddedSizes {
                    intercept: param(AppendStringMemIntercept),
                    slope: param(AppendStringMemSlope),
                }),
                cpu: TwoArguments::AddedSizes(AddedSizes {
                    intercept: param(AppendStringCpuIntercept),
                    slope: param(AppendStringCpuSlope),
                }),
            },
            b_data: Costing {
                mem: OneArgument::Constant(param(BDataMem)),
                cpu: OneArgument::Constant(param(BDataCpu)),
            },
            blake2b_256: Costing {
                mem: OneArgument::Constant(param(Blake2b256Mem)),
                cpu: OneArgument::LinearInX(LinearSize {
                    intercept: param(Blake2b256CpuIntercept),
                    slope: param(Blake2b256CpuSlope),
                }),
            },
            choose_data: Costing {
                mem: SixArguments::Constant(param(ChooseDataMem)),
                cpu: SixArguments::Constant(param(ChooseDataCpu)),
            },
            choose_list: Costing {
                mem: ThreeArguments::Constant(param(ChooseListMem)),
                cpu: ThreeArguments::Constant(param(ChooseListCpu)),
            },
            choose_unit: Costing {
                mem: TwoArguments::Constant(param(ChooseUnitMem)),
                cpu: TwoArguments::Constant(param(ChooseUnitCpu)),
            },
            cons_byte_string: Costing {
                mem: TwoArguments::AddedSizes(AddedSizes {
                    intercept: param(ConsByteStringMemIntercept),
                    slope: param(ConsByteStringMemSlope),
                }),
                cpu: TwoArguments::LinearInY(LinearSize {
                    intercept: param(ConsByteStringCpuIntercept),
                    slope: param(ConsByteStringCpuSlope),
                }),
            },
            constr_data: Costing {
                mem: TwoArguments::Constant(param(ConstrDataMem)),
                cpu: TwoArguments::Constant(param(ConstrDataCpu)),
            },
            decode_utf8: Costing {
                mem: OneArgument::LinearInX(LinearSize {
                    intercept: param(DecodeUtf8MemIntercept),
                    slope: param(DecodeUtf8MemSlope),
                }),
                cpu: OneArgument::LinearInX(LinearSize {
                    intercept: param(DecodeUtf8CpuIntercept),
                    slope: param(DecodeUtf8CpuSlope),
                }),
            },
            divide_integer: Costing {
                mem: TwoArguments::SubtractedSizes(SubtractedSizes {
                    intercept: param(DivideIntegerMemIntercept),
                    slope: param(DivideIntegerMemSlope),
                    minimum: param(DivideIntegerMemMinimum),
                }),
                cpu: match semantics {
                    Semantics::A | Semantics::B => TwoArguments::ConstAboveDiagonal(
                        param(DivideIntegerCpuConstant),
                        Box::new(TwoArguments::MultipliedSizes(MultipliedSizes {
                            intercept: param(DivideIntegerCpuIntercept),
                            slope: param(DivideIntegerCpuSlope),
                        })),
                    ),
                    Semantics::D => {
                        TwoArguments::AboveAndBelowDiagonal(Box::new(TwoArguments::MultipliedSizes(MultipliedSizes {
                            intercept: param(DivideIntegerCpuIntercept),
                            slope: param(DivideIntegerCpuSlope),
                        })))
                    }
                    Semantics::C => TwoArguments::ConstAboveDiagonal(
                        param(DivideIntegerCpuConstant),
                        Box::new(TwoArguments::QuadraticInXAndY(TwoArgumentsQuadraticFunction {
                            minimum: param(DivideIntegerCpuMinimum),
                            coeff_00: param(DivideIntegerCpuC00),
                            coeff_10: param(DivideIntegerCpuC10),
                            coeff_01: param(DivideIntegerCpuC01),
                            coeff_20: param(DivideIntegerCpuC20),
                            coeff_11: param(DivideIntegerCpuC11),
                            coeff_02: param(DivideIntegerCpuC02),
                        })),
                    ),
                    Semantics::E => TwoArguments::AboveAndBelowDiagonal(Box::new(TwoArguments::QuadraticInXAndY(
                        TwoArgumentsQuadraticFunction {
                            minimum: param(DivideIntegerCpuMinimum),
                            coeff_00: param(DivideIntegerCpuC00),
                            coeff_10: param(DivideIntegerCpuC10),
                            coeff_01: param(DivideIntegerCpuC01),
                            coeff_20: param(DivideIntegerCpuC20),
                            coeff_11: param(DivideIntegerCpuC11),
                            coeff_02: param(DivideIntegerCpuC02),
                        },
                    ))),
                },
            },
            encode_utf8: Costing {
                mem: OneArgument::LinearInX(LinearSize {
                    intercept: param(EncodeUtf8MemIntercept),
                    slope: param(EncodeUtf8MemSlope),
                }),
                cpu: OneArgument::LinearInX(LinearSize {
                    intercept: param(EncodeUtf8CpuIntercept),
                    slope: param(EncodeUtf8CpuSlope),
                }),
            },
            equals_byte_string: Costing {
                mem: TwoArguments::Constant(param(EqualsByteStringMem)),
                cpu: TwoArguments::LinearOnDiagonal(ConstantOrLinear {
                    constant: param(EqualsByteStringCpuConstant),
                    intercept: param(EqualsByteStringCpuIntercept),
                    slope: param(EqualsByteStringCpuSlope),
                }),
            },
            equals_data: Costing {
                mem: TwoArguments::Constant(param(EqualsDataMem)),
                cpu: TwoArguments::MinSize(MinSize {
                    intercept: param(EqualsDataCpuIntercept),
                    slope: param(EqualsDataCpuSlope),
                }),
            },
            equals_integer: Costing {
                mem: TwoArguments::Constant(param(EqualsIntegerMem)),
                cpu: TwoArguments::MinSize(MinSize {
                    intercept: param(EqualsIntegerCpuIntercept),
                    slope: param(EqualsIntegerCpuSlope),
                }),
            },
            equals_string: Costing {
                mem: TwoArguments::Constant(param(EqualsStringMem)),
                cpu: TwoArguments::LinearOnDiagonal(ConstantOrLinear {
                    constant: param(EqualsStringCpuConstant),
                    intercept: param(EqualsStringCpuIntercept),
                    slope: param(EqualsStringCpuSlope),
                }),
            },
            fst_pair: Costing {
                mem: OneArgument::Constant(param(FstPairMem)),
                cpu: OneArgument::Constant(param(FstPairCpu)),
            },
            head_list: Costing {
                mem: OneArgument::Constant(param(HeadListMem)),
                cpu: OneArgument::Constant(param(HeadListCpu)),
            },
            i_data: Costing {
                mem: OneArgument::Constant(param(IDataMem)),
                cpu: OneArgument::Constant(param(IDataCpu)),
            },
            if_then_else: Costing {
                mem: ThreeArguments::Constant(param(IfThenElseMem)),
                cpu: ThreeArguments::Constant(param(IfThenElseCpu)),
            },
            index_byte_string: Costing {
                mem: TwoArguments::Constant(param(IndexByteStringMem)),
                cpu: TwoArguments::Constant(param(IndexByteStringCpu)),
            },
            length_of_byte_string: Costing {
                mem: OneArgument::Constant(param(LengthOfByteStringMem)),
                cpu: OneArgument::Constant(param(LengthOfByteStringCpu)),
            },
            less_than_byte_string: Costing {
                mem: TwoArguments::Constant(param(LessThanByteStringMem)),
                cpu: TwoArguments::MinSize(MinSize {
                    intercept: param(LessThanByteStringCpuIntercept),
                    slope: param(LessThanByteStringCpuSlope),
                }),
            },
            less_than_equals_byte_string: Costing {
                mem: TwoArguments::Constant(param(LessThanEqualsByteStringMem)),
                cpu: TwoArguments::MinSize(MinSize {
                    intercept: param(LessThanEqualsByteStringCpuIntercept),
                    slope: param(LessThanEqualsByteStringCpuSlope),
                }),
            },
            less_than_equals_integer: Costing {
                mem: TwoArguments::Constant(param(LessThanEqualsIntegerMem)),
                cpu: TwoArguments::MinSize(MinSize {
                    intercept: param(LessThanEqualsIntegerCpuIntercept),
                    slope: param(LessThanEqualsIntegerCpuSlope),
                }),
            },
            less_than_integer: Costing {
                mem: TwoArguments::Constant(param(LessThanIntegerMem)),
                cpu: TwoArguments::MinSize(MinSize {
                    intercept: param(LessThanIntegerCpuIntercept),
                    slope: param(LessThanIntegerCpuSlope),
                }),
            },
            list_data: Costing {
                mem: OneArgument::Constant(param(ListDataMem)),
                cpu: OneArgument::Constant(param(ListDataCpu)),
            },
            map_data: Costing {
                mem: OneArgument::Constant(param(MapDataMem)),
                cpu: OneArgument::Constant(param(MapDataCpu)),
            },
            mk_cons: Costing {
                mem: TwoArguments::Constant(param(MkConsMem)),
                cpu: TwoArguments::Constant(param(MkConsCpu)),
            },
            mk_nil_data: Costing {
                mem: OneArgument::Constant(param(MkNilDataMem)),
                cpu: OneArgument::Constant(param(MkNilDataCpu)),
            },
            mk_nil_pair_data: Costing {
                mem: OneArgument::Constant(param(MkNilPairDataMem)),
                cpu: OneArgument::Constant(param(MkNilPairDataCpu)),
            },
            mk_pair_data: Costing {
                mem: TwoArguments::Constant(param(MkPairDataMem)),
                cpu: TwoArguments::Constant(param(MkPairDataCpu)),
            },
            mod_integer: Costing {
                mem: match semantics {
                    Semantics::A | Semantics::B => TwoArguments::SubtractedSizes(SubtractedSizes {
                        intercept: param(ModIntegerMemIntercept),
                        minimum: param(ModIntegerMemMinimum),
                        slope: param(ModIntegerMemSlope),
                    }),
                    Semantics::C | Semantics::D | Semantics::E => TwoArguments::LinearInY(LinearSize {
                        intercept: param(ModIntegerMemIntercept),
                        slope: param(ModIntegerMemSlope),
                    }),
                },
                cpu: match semantics {
                    Semantics::A | Semantics::B => TwoArguments::ConstAboveDiagonal(
                        param(ModIntegerCpuConstant),
                        Box::new(TwoArguments::MultipliedSizes(MultipliedSizes {
                            intercept: param(ModIntegerCpuIntercept),
                            slope: param(ModIntegerCpuSlope),
                        })),
                    ),
                    Semantics::D => {
                        TwoArguments::AboveAndBelowDiagonal(Box::new(TwoArguments::MultipliedSizes(MultipliedSizes {
                            intercept: param(ModIntegerCpuIntercept),
                            slope: param(ModIntegerCpuSlope),
                        })))
                    }
                    Semantics::C => TwoArguments::ConstAboveDiagonal(
                        param(ModIntegerCpuConstant),
                        Box::new(TwoArguments::QuadraticInXAndY(TwoArgumentsQuadraticFunction {
                            minimum: param(ModIntegerCpuMinimum),
                            coeff_00: param(ModIntegerCpuC00),
                            coeff_10: param(ModIntegerCpuC10),
                            coeff_01: param(ModIntegerCpuC01),
                            coeff_20: param(ModIntegerCpuC20),
                            coeff_11: param(ModIntegerCpuC11),
                            coeff_02: param(ModIntegerCpuC02),
                        })),
                    ),
                    Semantics::E => TwoArguments::AboveAndBelowDiagonal(Box::new(TwoArguments::QuadraticInXAndY(
                        TwoArgumentsQuadraticFunction {
                            minimum: param(ModIntegerCpuMinimum),
                            coeff_00: param(ModIntegerCpuC00),
                            coeff_10: param(ModIntegerCpuC10),
                            coeff_01: param(ModIntegerCpuC01),
                            coeff_20: param(ModIntegerCpuC20),
                            coeff_11: param(ModIntegerCpuC11),
                            coeff_02: param(ModIntegerCpuC02),
                        },
                    ))),
                },
            },
            multiply_integer: Costing {
                mem: TwoArguments::AddedSizes(AddedSizes {
                    intercept: param(MultiplyIntegerMemIntercept),
                    slope: param(MultiplyIntegerMemSlope),
                }),
                cpu: match semantics {
                    Semantics::A => TwoArguments::AddedSizes(AddedSizes {
                        intercept: param(MultiplyIntegerCpuIntercept),
                        slope: param(MultiplyIntegerCpuSlope),
                    }),
                    Semantics::B | Semantics::C | Semantics::D | Semantics::E => {
                        TwoArguments::MultipliedSizes(MultipliedSizes {
                            intercept: param(MultiplyIntegerCpuIntercept),
                            slope: param(MultiplyIntegerCpuSlope),
                        })
                    }
                },
            },
            null_list: Costing {
                mem: OneArgument::Constant(param(NullListMem)),
                cpu: OneArgument::Constant(param(NullListCpu)),
            },
            quotient_integer: Costing {
                mem: TwoArguments::SubtractedSizes(SubtractedSizes {
                    intercept: param(QuotientIntegerMemIntercept),
                    slope: param(QuotientIntegerMemSlope),
                    minimum: param(QuotientIntegerMemMinimum),
                }),
                cpu: match semantics {
                    Semantics::A | Semantics::B | Semantics::D => TwoArguments::ConstAboveDiagonal(
                        param(QuotientIntegerCpuConstant),
                        Box::new(TwoArguments::MultipliedSizes(MultipliedSizes {
                            intercept: param(QuotientIntegerCpuIntercept),
                            slope: param(QuotientIntegerCpuSlope),
                        })),
                    ),
                    Semantics::C | Semantics::E => TwoArguments::ConstAboveDiagonal(
                        param(QuotientIntegerCpuConstant),
                        Box::new(TwoArguments::QuadraticInXAndY(TwoArgumentsQuadraticFunction {
                            minimum: param(QuotientIntegerCpuMinimum),
                            coeff_00: param(QuotientIntegerCpuC00),
                            coeff_10: param(QuotientIntegerCpuC10),
                            coeff_01: param(QuotientIntegerCpuC01),
                            coeff_20: param(QuotientIntegerCpuC20),
                            coeff_11: param(QuotientIntegerCpuC11),
                            coeff_02: param(QuotientIntegerCpuC02),
                        })),
                    ),
                },
            },
            remainder_integer: Costing {
                mem: match semantics {
                    Semantics::A | Semantics::B => TwoArguments::SubtractedSizes(SubtractedSizes {
                        intercept: param(RemainderIntegerMemIntercept),
                        minimum: param(RemainderIntegerMemMinimum),
                        slope: param(RemainderIntegerMemSlope),
                    }),
                    Semantics::C | Semantics::D | Semantics::E => TwoArguments::LinearInY(LinearSize {
                        intercept: param(RemainderIntegerMemIntercept),
                        slope: param(RemainderIntegerMemSlope),
                    }),
                },
                cpu: match semantics {
                    Semantics::A | Semantics::B | Semantics::D => TwoArguments::ConstAboveDiagonal(
                        param(RemainderIntegerCpuConstant),
                        Box::new(TwoArguments::MultipliedSizes(MultipliedSizes {
                            intercept: param(RemainderIntegerCpuIntercept),
                            slope: param(RemainderIntegerCpuSlope),
                        })),
                    ),
                    Semantics::C | Semantics::E => TwoArguments::ConstAboveDiagonal(
                        param(RemainderIntegerCpuConstant),
                        Box::new(TwoArguments::QuadraticInXAndY(TwoArgumentsQuadraticFunction {
                            minimum: param(RemainderIntegerCpuMinimum),
                            coeff_00: param(RemainderIntegerCpuC00),
                            coeff_10: param(RemainderIntegerCpuC10),
                            coeff_01: param(RemainderIntegerCpuC01),
                            coeff_20: param(RemainderIntegerCpuC20),
                            coeff_11: param(RemainderIntegerCpuC11),
                            coeff_02: param(RemainderIntegerCpuC02),
                        })),
                    ),
                },
            },
            serialise_data: Costing {
                mem: OneArgument::LinearInX(LinearSize {
                    intercept: param(SerialiseDataMemIntercept),
                    slope: param(SerialiseDataMemSlope),
                }),
                cpu: OneArgument::LinearInX(LinearSize {
                    intercept: param(SerialiseDataCpuIntercept),
                    slope: param(SerialiseDataCpuSlope),
                }),
            },
            sha2_256: Costing {
                mem: OneArgument::Constant(param(Sha2256Mem)),
                cpu: OneArgument::LinearInX(LinearSize {
                    intercept: param(Sha2256CpuIntercept),
                    slope: param(Sha2256CpuSlope),
                }),
            },
            sha3_256: Costing {
                mem: OneArgument::Constant(param(Sha3256Mem)),
                cpu: OneArgument::LinearInX(LinearSize {
                    intercept: param(Sha3256CpuIntercept),
                    slope: param(Sha3256CpuSlope),
                }),
            },
            slice_byte_string: Costing {
                mem: ThreeArguments::LinearInZ(LinearSize {
                    intercept: param(SliceByteStringMemIntercept),
                    slope: param(SliceByteStringMemSlope),
                }),
                cpu: ThreeArguments::LinearInZ(LinearSize {
                    intercept: param(SliceByteStringCpuIntercept),
                    slope: param(SliceByteStringCpuSlope),
                }),
            },
            snd_pair: Costing {
                mem: OneArgument::Constant(param(SndPairMem)),
                cpu: OneArgument::Constant(param(SndPairCpu)),
            },
            subtract_integer: Costing {
                mem: TwoArguments::MaxSize(MaxSize {
                    intercept: param(SubtractIntegerMemIntercept),
                    slope: param(SubtractIntegerMemSlope),
                }),
                cpu: TwoArguments::MaxSize(MaxSize {
                    intercept: param(SubtractIntegerCpuIntercept),
                    slope: param(SubtractIntegerCpuSlope),
                }),
            },
            tail_list: Costing {
                mem: OneArgument::Constant(param(TailListMem)),
                cpu: OneArgument::Constant(param(TailListCpu)),
            },
            trace: Costing {
                mem: TwoArguments::Constant(param(TraceMem)),
                cpu: TwoArguments::Constant(param(TraceCpu)),
            },
            un_b_data: Costing {
                mem: OneArgument::Constant(param(UnBDataMem)),
                cpu: OneArgument::Constant(param(UnBDataCpu)),
            },
            un_constr_data: Costing {
                mem: OneArgument::Constant(param(UnConstrDataMem)),
                cpu: OneArgument::Constant(param(UnConstrDataCpu)),
            },
            un_i_data: Costing {
                mem: OneArgument::Constant(param(UnIDataMem)),
                cpu: OneArgument::Constant(param(UnIDataCpu)),
            },
            un_list_data: Costing {
                mem: OneArgument::Constant(param(UnListDataMem)),
                cpu: OneArgument::Constant(param(UnListDataCpu)),
            },
            un_map_data: Costing {
                mem: OneArgument::Constant(param(UnMapDataMem)),
                cpu: OneArgument::Constant(param(UnMapDataCpu)),
            },
            verify_ecdsa_secp256k1_signature: Costing {
                mem: ThreeArguments::Constant(param(VerifyEcdsaSecp256k1SignatureMem)),
                cpu: ThreeArguments::Constant(param(VerifyEcdsaSecp256k1SignatureCpu)),
            },
            verify_ed25519_signature: Costing {
                mem: ThreeArguments::Constant(param(VerifyEd25519SignatureMem)),
                cpu: match semantics {
                    Semantics::A => ThreeArguments::LinearInZ(LinearSize {
                        intercept: param(VerifyEd25519SignatureCpuIntercept),
                        slope: param(VerifyEd25519SignatureCpuSlope),
                    }),
                    Semantics::B | Semantics::C | Semantics::D | Semantics::E => {
                        ThreeArguments::LinearInY(LinearSize {
                            intercept: param(VerifyEd25519SignatureCpuIntercept),
                            slope: param(VerifyEd25519SignatureCpuSlope),
                        })
                    }
                },
            },
            verify_schnorr_secp256k1_signature: Costing {
                mem: ThreeArguments::Constant(param(VerifySchnorrSecp256k1SignatureMem)),
                cpu: ThreeArguments::LinearInY(LinearSize {
                    intercept: param(VerifySchnorrSecp256k1SignatureCpuIntercept),
                    slope: param(VerifySchnorrSecp256k1SignatureCpuSlope),
                }),
            },
            bls12_381_g1_add: Costing {
                cpu: TwoArguments::Constant(param(BlsG1AddCpu)),
                mem: TwoArguments::Constant(param(BlsG1AddMem)),
            },
            bls12_381_g1_compress: Costing {
                cpu: OneArgument::Constant(param(BlsG1CompressCpu)),
                mem: OneArgument::Constant(param(BlsG1CompressMem)),
            },
            bls12_381_g1_equal: Costing {
                cpu: TwoArguments::Constant(param(BlsG1EqualCpu)),
                mem: TwoArguments::Constant(param(BlsG1EqualMem)),
            },
            bls12_381_g1_hash_to_group: Costing {
                cpu: TwoArguments::LinearInX(LinearSize {
                    intercept: param(BlsG1HashToGroupCpuIntercept),
                    slope: param(BlsG1HashToGroupCpuSlope),
                }),
                mem: TwoArguments::Constant(param(BlsG1HashToGroupMem)),
            },
            bls12_381_g1_neg: Costing {
                cpu: OneArgument::Constant(param(BlsG1NegCpu)),
                mem: OneArgument::Constant(param(BlsG1NegMem)),
            },
            bls12_381_g1_scalar_mul: Costing {
                cpu: TwoArguments::LinearInX(LinearSize {
                    intercept: param(BlsG1ScalarMulCpuIntercept),
                    slope: param(BlsG1ScalarMulCpuSlope),
                }),
                mem: TwoArguments::Constant(param(BlsG1ScalarMulMem)),
            },
            bls12_381_g1_uncompress: Costing {
                cpu: OneArgument::Constant(param(BlsG1UncompressCpu)),
                mem: OneArgument::Constant(param(BlsG1UncompressMem)),
            },
            bls12_381_g2_add: Costing {
                cpu: TwoArguments::Constant(param(BlsG2AddCpu)),
                mem: TwoArguments::Constant(param(BlsG2AddMem)),
            },
            bls12_381_g2_compress: Costing {
                cpu: OneArgument::Constant(param(BlsG2CompressCpu)),
                mem: OneArgument::Constant(param(BlsG2CompressMem)),
            },
            bls12_381_g2_equal: Costing {
                cpu: TwoArguments::Constant(param(BlsG2EqualCpu)),
                mem: TwoArguments::Constant(param(BlsG2EqualMem)),
            },
            bls12_381_g2_hash_to_group: Costing {
                cpu: TwoArguments::LinearInX(LinearSize {
                    intercept: param(BlsG2HashToGroupCpuIntercept),
                    slope: param(BlsG2HashToGroupCpuSlope),
                }),
                mem: TwoArguments::Constant(param(BlsG2HashToGroupMem)),
            },
            bls12_381_g2_neg: Costing {
                cpu: OneArgument::Constant(param(BlsG2NegCpu)),
                mem: OneArgument::Constant(param(BlsG2NegMem)),
            },
            bls12_381_g2_scalar_mul: Costing {
                cpu: TwoArguments::LinearInX(LinearSize {
                    intercept: param(BlsG2ScalarMulCpuIntercept),
                    slope: param(BlsG2ScalarMulCpuSlope),
                }),
                mem: TwoArguments::Constant(param(BlsG2ScalarMulMem)),
            },
            bls12_381_g2_uncompress: Costing {
                cpu: OneArgument::Constant(param(BlsG2UncompressCpu)),
                mem: OneArgument::Constant(param(BlsG2UncompressMem)),
            },
            bls12_381_final_verify: Costing {
                cpu: TwoArguments::Constant(param(BlsFinalVerifyCpu)),
                mem: TwoArguments::Constant(param(BlsFinalVerifyMem)),
            },
            bls12_381_miller_loop: Costing {
                cpu: TwoArguments::Constant(param(BlsMillerLoopCpu)),
                mem: TwoArguments::Constant(param(BlsMillerLoopMem)),
            },
            bls12_381_mul_ml_result: Costing {
                cpu: TwoArguments::Constant(param(BlsMulMlResultCpu)),
                mem: TwoArguments::Constant(param(BlsMulMlResultMem)),
            },
            keccak_256: Costing {
                cpu: OneArgument::LinearInX(LinearSize {
                    intercept: param(Keccak256CpuIntercept),
                    slope: param(Keccak256CpuSlope),
                }),
                mem: OneArgument::Constant(param(Keccak256Mem)),
            },
            blake2b_224: Costing {
                cpu: OneArgument::LinearInX(LinearSize {
                    intercept: param(Blake2b224CpuIntercept),
                    slope: param(Blake2b224CpuSlope),
                }),
                mem: OneArgument::Constant(param(Blake2b224Mem)),
            },
            integer_to_byte_string: Costing {
                cpu: ThreeArguments::QuadraticInZ(QuadraticFunction {
                    coeff_0: param(IntegerToByteStringCpuC0),
                    coeff_1: param(IntegerToByteStringCpuC1),
                    coeff_2: param(IntegerToByteStringCpuC2),
                }),
                mem: ThreeArguments::LiteralInYorLinearInZ(LinearSize {
                    intercept: param(IntegerToByteStringMemIntercept),
                    slope: param(IntegerToByteStringMemSlope),
                }),
            },
            byte_string_to_integer: Costing {
                cpu: TwoArguments::QuadraticInY(QuadraticFunction {
                    coeff_0: param(ByteStringToIntegerCpuC0),
                    coeff_1: param(ByteStringToIntegerCpuC1),
                    coeff_2: param(ByteStringToIntegerCpuC2),
                }),
                mem: TwoArguments::LinearInY(LinearSize {
                    intercept: param(ByteStringToIntegerMemIntercept),
                    slope: param(ByteStringToIntegerMemSlope),
                }),
            },

            // Starting from ProtocolVersion >= 10
            and_byte_string: Costing {
                cpu: ThreeArguments::LinearInYAndZ(TwoVariableLinearSize {
                    intercept: param(AndByteStringCpuIntercept),
                    slope1: param(AndByteStringCpuSlope1),
                    slope2: param(AndByteStringCpuSlope2),
                }),
                mem: ThreeArguments::LinearInMaxYZ(LinearSize {
                    intercept: param(AndByteStringMemIntercept),
                    slope: param(AndByteStringMemSlope),
                }),
            },
            or_byte_string: Costing {
                cpu: ThreeArguments::LinearInYAndZ(TwoVariableLinearSize {
                    intercept: param(OrByteStringCpuIntercept),
                    slope1: param(OrByteStringCpuSlope1),
                    slope2: param(OrByteStringCpuSlope2),
                }),
                mem: ThreeArguments::LinearInMaxYZ(LinearSize {
                    intercept: param(OrByteStringMemIntercept),
                    slope: param(OrByteStringMemSlope),
                }),
            },
            xor_byte_string: Costing {
                cpu: ThreeArguments::LinearInYAndZ(TwoVariableLinearSize {
                    intercept: param(XorByteStringCpuIntercept),
                    slope1: param(XorByteStringCpuSlope1),
                    slope2: param(XorByteStringCpuSlope2),
                }),
                mem: ThreeArguments::LinearInMaxYZ(LinearSize {
                    intercept: param(XorByteStringMemIntercept),
                    slope: param(XorByteStringMemSlope),
                }),
            },
            complement_byte_string: Costing {
                cpu: OneArgument::LinearInX(LinearSize {
                    intercept: param(ComplementByteStringCpuIntercept),
                    slope: param(ComplementByteStringCpuSlope),
                }),
                mem: OneArgument::LinearInX(LinearSize {
                    intercept: param(ComplementByteStringMemIntercept),
                    slope: param(ComplementByteStringMemSlope),
                }),
            },
            read_bit: Costing {
                cpu: TwoArguments::Constant(param(ReadBitCpu)),
                mem: TwoArguments::Constant(param(ReadBitMem)),
            },
            write_bits: Costing {
                cpu: ThreeArguments::LinearInY(LinearSize {
                    intercept: param(WriteBitsCpuIntercept),
                    slope: param(WriteBitsCpuSlope),
                }),
                mem: ThreeArguments::LinearInX(LinearSize {
                    intercept: param(WriteBitsMemIntercept),
                    slope: param(WriteBitsMemSlope),
                }),
            },
            replicate_byte: Costing {
                cpu: TwoArguments::LinearInX(LinearSize {
                    intercept: param(ReplicateByteCpuIntercept),
                    slope: param(ReplicateByteCpuSlope),
                }),
                mem: TwoArguments::LinearInX(LinearSize {
                    intercept: param(ReplicateByteMemIntercept),
                    slope: param(ReplicateByteMemSlope),
                }),
            },
            shift_byte_string: Costing {
                cpu: TwoArguments::LinearInX(LinearSize {
                    intercept: param(ShiftByteStringCpuIntercept),
                    slope: param(ShiftByteStringCpuSlope),
                }),
                mem: TwoArguments::LinearInX(LinearSize {
                    intercept: param(ShiftByteStringMemIntercept),
                    slope: param(ShiftByteStringMemSlope),
                }),
            },
            rotate_byte_string: Costing {
                cpu: TwoArguments::LinearInX(LinearSize {
                    intercept: param(RotateByteStringCpuIntercept),
                    slope: param(RotateByteStringCpuSlope),
                }),
                mem: TwoArguments::LinearInX(LinearSize {
                    intercept: param(RotateByteStringMemIntercept),
                    slope: param(RotateByteStringMemSlope),
                }),
            },
            count_set_bits: Costing {
                cpu: OneArgument::LinearInX(LinearSize {
                    intercept: param(CountSetBitsCpuIntercept),
                    slope: param(CountSetBitsCpuSlope),
                }),
                mem: OneArgument::Constant(param(CountSetBitsMem)),
            },
            find_first_set_bit: Costing {
                cpu: OneArgument::LinearInX(LinearSize {
                    intercept: param(FindFirstSetBitCpuIntercept),
                    slope: param(FindFirstSetBitCpuSlope),
                }),
                mem: OneArgument::Constant(param(FindFirstSetBitMem)),
            },
            ripemd_160: Costing {
                cpu: OneArgument::LinearInX(LinearSize {
                    intercept: param(Ripemd160CpuIntercept),
                    slope: param(Ripemd160CpuSlope),
                }),
                mem: OneArgument::Constant(param(Ripemd160Mem)),
            },

            // Starting from ProtocolVersion >= 11
            exp_mod_integer: Costing {
                cpu: ThreeArguments::ExpModCost(ExpModCost {
                    coeff_00: param(ExpModIntegerCpuC00),
                    coeff_11: param(ExpModIntegerCpuC11),
                    coeff_12: param(ExpModIntegerCpuC12),
                }),
                mem: ThreeArguments::LinearInZ(LinearSize {
                    intercept: param(ExpModIntegerMemIntercept),
                    slope: param(ExpModIntegerMemSlope),
                }),
            },
            drop_list: Costing {
                cpu: TwoArguments::LinearInX(LinearSize {
                    intercept: param(DropListCpuIntercept),
                    slope: param(DropListCpuSlope),
                }),
                mem: TwoArguments::Constant(param(DropListMem)),
            },
            length_of_array: Costing {
                cpu: OneArgument::Constant(param(LengthOfArrayCpu)),
                mem: OneArgument::Constant(param(LengthOfArrayMem)),
            },
            list_to_array: Costing {
                cpu: OneArgument::LinearInX(LinearSize {
                    intercept: param(ListToArrayCpuIntercept),
                    slope: param(ListToArrayCpuSlope),
                }),
                mem: OneArgument::LinearInX(LinearSize {
                    intercept: param(ListToArrayMemIntercept),
                    slope: param(ListToArrayMemSlope),
                }),
            },
            index_array: Costing {
                cpu: TwoArguments::Constant(param(IndexArrayCpu)),
                mem: TwoArguments::Constant(param(IndexArrayMem)),
            },
            bls12_381_g1_multi_scalar_mul: Costing {
                cpu: TwoArguments::LinearInX(LinearSize {
                    intercept: param(BlsG1MultiScalarMulCpuIntercept),
                    slope: param(BlsG1MultiScalarMulCpuSlope),
                }),
                mem: TwoArguments::Constant(param(BlsG1MultiScalarMulMem)),
            },
            bls12_381_g2_multi_scalar_mul: Costing {
                cpu: TwoArguments::LinearInX(LinearSize {
                    intercept: param(BlsG2MultiScalarMulCpuIntercept),
                    slope: param(BlsG2MultiScalarMulCpuSlope),
                }),
                mem: TwoArguments::Constant(param(BlsG2MultiScalarMulMem)),
            },
            insert_coin: Costing {
                cpu: FourArguments::LinearInU(LinearSize {
                    intercept: param(InsertCoinCpuIntercept),
                    slope: param(InsertCoinCpuSlope),
                }),
                mem: FourArguments::LinearInU(LinearSize {
                    intercept: param(InsertCoinMemIntercept),
                    slope: param(InsertCoinMemSlope),
                }),
            },
            lookup_coin: Costing {
                cpu: ThreeArguments::LinearInZ(LinearSize {
                    intercept: param(LookupCoinCpuIntercept),
                    slope: param(LookupCoinCpuSlope),
                }),
                mem: ThreeArguments::Constant(param(LookupCoinMem)),
            },
            union_value: Costing {
                cpu: TwoArguments::WithInteraction(WithInteraction {
                    coeff_00: param(UnionValueCpuC00),
                    coeff_10: param(UnionValueCpuC10),
                    coeff_01: param(UnionValueCpuC01),
                    coeff_11: param(UnionValueCpuC11),
                }),
                mem: TwoArguments::AddedSizes(AddedSizes {
                    intercept: param(UnionValueMemIntercept),
                    slope: param(UnionValueMemSlope),
                }),
            },
            value_contains: Costing {
                cpu: TwoArguments::ConstAboveDiagonal(
                    param(ValueContainsCpuConstant),
                    Box::new(TwoArguments::LinearInXAndY(TwoVariableLinearSize {
                        intercept: param(ValueContainsCpuIntercept),
                        slope1: param(ValueContainsCpuSlope1),
                        slope2: param(ValueContainsCpuSlope2),
                    })),
                ),
                mem: TwoArguments::Constant(param(ValueContainsMem)),
            },
            value_data: Costing {
                cpu: OneArgument::LinearInX(LinearSize {
                    intercept: param(ValueDataCpuIntercept),
                    slope: param(ValueDataCpuSlope),
                }),
                mem: OneArgument::LinearInX(LinearSize {
                    intercept: param(ValueDataMemIntercept),
                    slope: param(ValueDataMemSlope),
                }),
            },
            un_value_data: Costing {
                cpu: OneArgument::Quadratic(QuadraticFunction {
                    coeff_0: param(UnValueDataCpuC0),
                    coeff_1: param(UnValueDataCpuC1),
                    coeff_2: param(UnValueDataCpuC2),
                }),
                mem: OneArgument::LinearInX(LinearSize {
                    intercept: param(UnValueDataMemIntercept),
                    slope: param(UnValueDataMemSlope),
                }),
            },
            scale_value: Costing {
                cpu: TwoArguments::LinearInY(LinearSize {
                    intercept: param(ScaleValueCpuIntercept),
                    slope: param(ScaleValueCpuSlope),
                }),
                mem: TwoArguments::LinearInY(LinearSize {
                    intercept: param(ScaleValueMemIntercept),
                    slope: param(ScaleValueMemSlope),
                }),
            },
        }
    }

    // TODO:
    // Cost should really be associated with the `DefaultFunction` themselves instead of re-pattern
    // matching here every time.
    pub fn get_cost(&self, builtin: DefaultFunction, args: &[CostArgument<'_>]) -> ExBudget {
        match builtin {
            DefaultFunction::AddInteger => cost(&self.add_integer, args),
            DefaultFunction::SubtractInteger => cost(&self.subtract_integer, args),
            DefaultFunction::MultiplyInteger => cost(&self.multiply_integer, args),
            DefaultFunction::DivideInteger => cost(&self.divide_integer, args),
            DefaultFunction::QuotientInteger => cost(&self.quotient_integer, args),
            DefaultFunction::RemainderInteger => cost(&self.remainder_integer, args),
            DefaultFunction::ModInteger => cost(&self.mod_integer, args),
            DefaultFunction::EqualsInteger => cost(&self.equals_integer, args),
            DefaultFunction::LessThanInteger => cost(&self.less_than_integer, args),
            DefaultFunction::LessThanEqualsInteger => cost(&self.less_than_equals_integer, args),
            DefaultFunction::AppendByteString => cost(&self.append_byte_string, args),
            DefaultFunction::ConsByteString => cost(&self.cons_byte_string, args),
            DefaultFunction::SliceByteString => cost(&self.slice_byte_string, args),
            DefaultFunction::LengthOfByteString => cost(&self.length_of_byte_string, args),
            DefaultFunction::IndexByteString => cost(&self.index_byte_string, args),
            DefaultFunction::EqualsByteString => cost(&self.equals_byte_string, args),
            DefaultFunction::LessThanByteString => cost(&self.less_than_byte_string, args),
            DefaultFunction::LessThanEqualsByteString => cost(&self.less_than_equals_byte_string, args),
            DefaultFunction::Sha2_256 => cost(&self.sha2_256, args),
            DefaultFunction::Sha3_256 => cost(&self.sha3_256, args),
            DefaultFunction::Blake2b_224 => cost(&self.blake2b_224, args),
            DefaultFunction::Blake2b_256 => cost(&self.blake2b_256, args),
            DefaultFunction::Keccak_256 => cost(&self.keccak_256, args),
            DefaultFunction::VerifyEd25519Signature => cost(&self.verify_ed25519_signature, args),
            DefaultFunction::VerifyEcdsaSecp256k1Signature => cost(&self.verify_ecdsa_secp256k1_signature, args),
            DefaultFunction::VerifySchnorrSecp256k1Signature => cost(&self.verify_schnorr_secp256k1_signature, args),
            DefaultFunction::AppendString => cost(&self.append_string, args),
            DefaultFunction::EqualsString => cost(&self.equals_string, args),
            DefaultFunction::EncodeUtf8 => cost(&self.encode_utf8, args),
            DefaultFunction::DecodeUtf8 => cost(&self.decode_utf8, args),
            DefaultFunction::IfThenElse => cost(&self.if_then_else, args),
            DefaultFunction::ChooseUnit => cost(&self.choose_unit, args),
            DefaultFunction::Trace => cost(&self.trace, args),
            DefaultFunction::FstPair => cost(&self.fst_pair, args),
            DefaultFunction::SndPair => cost(&self.snd_pair, args),
            DefaultFunction::ChooseList => cost(&self.choose_list, args),
            DefaultFunction::MkCons => cost(&self.mk_cons, args),
            DefaultFunction::HeadList => cost(&self.head_list, args),
            DefaultFunction::TailList => cost(&self.tail_list, args),
            DefaultFunction::NullList => cost(&self.null_list, args),
            DefaultFunction::ChooseData => cost(&self.choose_data, args),
            DefaultFunction::ConstrData => cost(&self.constr_data, args),
            DefaultFunction::MapData => cost(&self.map_data, args),
            DefaultFunction::ListData => cost(&self.list_data, args),
            DefaultFunction::IData => cost(&self.i_data, args),
            DefaultFunction::BData => cost(&self.b_data, args),
            DefaultFunction::UnConstrData => cost(&self.un_constr_data, args),
            DefaultFunction::UnMapData => cost(&self.un_map_data, args),
            DefaultFunction::UnListData => cost(&self.un_list_data, args),
            DefaultFunction::UnIData => cost(&self.un_i_data, args),
            DefaultFunction::UnBData => cost(&self.un_b_data, args),
            DefaultFunction::EqualsData => cost(&self.equals_data, args),
            DefaultFunction::MkPairData => cost(&self.mk_pair_data, args),
            DefaultFunction::MkNilData => cost(&self.mk_nil_data, args),
            DefaultFunction::MkNilPairData => cost(&self.mk_nil_pair_data, args),
            DefaultFunction::SerialiseData => cost(&self.serialise_data, args),
            DefaultFunction::Bls12_381_G1_Add => cost(&self.bls12_381_g1_add, args),
            DefaultFunction::Bls12_381_G1_Neg => cost(&self.bls12_381_g1_neg, args),
            DefaultFunction::Bls12_381_G1_ScalarMul => cost(&self.bls12_381_g1_scalar_mul, args),
            DefaultFunction::Bls12_381_G1_Equal => cost(&self.bls12_381_g1_equal, args),
            DefaultFunction::Bls12_381_G1_Compress => cost(&self.bls12_381_g1_compress, args),
            DefaultFunction::Bls12_381_G1_Uncompress => cost(&self.bls12_381_g1_uncompress, args),
            DefaultFunction::Bls12_381_G1_HashToGroup => cost(&self.bls12_381_g1_hash_to_group, args),
            DefaultFunction::Bls12_381_G2_Add => cost(&self.bls12_381_g2_add, args),
            DefaultFunction::Bls12_381_G2_Neg => cost(&self.bls12_381_g2_neg, args),
            DefaultFunction::Bls12_381_G2_ScalarMul => cost(&self.bls12_381_g2_scalar_mul, args),
            DefaultFunction::Bls12_381_G2_Equal => cost(&self.bls12_381_g2_equal, args),
            DefaultFunction::Bls12_381_G2_Compress => cost(&self.bls12_381_g2_compress, args),
            DefaultFunction::Bls12_381_G2_Uncompress => cost(&self.bls12_381_g2_uncompress, args),
            DefaultFunction::Bls12_381_G2_HashToGroup => cost(&self.bls12_381_g2_hash_to_group, args),
            DefaultFunction::Bls12_381_MillerLoop => cost(&self.bls12_381_miller_loop, args),
            DefaultFunction::Bls12_381_MulMlResult => cost(&self.bls12_381_mul_ml_result, args),
            DefaultFunction::Bls12_381_FinalVerify => cost(&self.bls12_381_final_verify, args),
            DefaultFunction::IntegerToByteString => cost(&self.integer_to_byte_string, args),
            DefaultFunction::ByteStringToInteger => cost(&self.byte_string_to_integer, args),
            DefaultFunction::AndByteString => cost(&self.and_byte_string, args),
            DefaultFunction::OrByteString => cost(&self.or_byte_string, args),
            DefaultFunction::XorByteString => cost(&self.xor_byte_string, args),
            DefaultFunction::ComplementByteString => cost(&self.complement_byte_string, args),
            DefaultFunction::ReadBit => cost(&self.read_bit, args),
            DefaultFunction::WriteBits => cost(&self.write_bits, args),
            DefaultFunction::ReplicateByte => cost(&self.replicate_byte, args),
            DefaultFunction::ShiftByteString => cost(&self.shift_byte_string, args),
            DefaultFunction::RotateByteString => cost(&self.rotate_byte_string, args),
            DefaultFunction::CountSetBits => cost(&self.count_set_bits, args),
            DefaultFunction::FindFirstSetBit => cost(&self.find_first_set_bit, args),
            DefaultFunction::Ripemd_160 => cost(&self.ripemd_160, args),
            DefaultFunction::ExpModInteger => cost(&self.exp_mod_integer, args),
            DefaultFunction::DropList => cost(&self.drop_list, args),
            DefaultFunction::LengthOfArray => cost(&self.length_of_array, args),
            DefaultFunction::ListToArray => cost(&self.list_to_array, args),
            DefaultFunction::IndexArray => cost(&self.index_array, args),
            DefaultFunction::Bls12_381_G1_MultiScalarMul => cost(&self.bls12_381_g1_multi_scalar_mul, args),
            DefaultFunction::Bls12_381_G2_MultiScalarMul => cost(&self.bls12_381_g2_multi_scalar_mul, args),
            DefaultFunction::InsertCoin => cost(&self.insert_coin, args),
            DefaultFunction::LookupCoin => cost(&self.lookup_coin, args),
            DefaultFunction::UnionValue => cost(&self.union_value, args),
            DefaultFunction::ValueContains => cost(&self.value_contains, args),
            DefaultFunction::ValueData => cost(&self.value_data, args),
            DefaultFunction::UnValueData => cost(&self.un_value_data, args),
            DefaultFunction::ScaleValue => cost(&self.scale_value, args),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::BuiltinCosts;
    use crate::{
        builtin::DefaultFunction,
        machine::cost_model::cost_argument::{CostArgument, IntoMachineSize},
    };

    struct CountedMachineSize<'a> {
        calls: &'a Cell<u8>,
        value: i64,
    }

    impl IntoMachineSize for CountedMachineSize<'_> {
        fn size(&self) -> i64 {
            self.calls.set(self.calls.get() + 1);
            self.value
        }
    }

    #[test]
    fn write_bits_measures_only_the_arguments_its_costs_use() {
        let x_calls = Cell::new(0);
        let y_calls = Cell::new(0);
        let z_calls = Cell::new(0);
        let x = CountedMachineSize { calls: &x_calls, value: 1 };
        let y = CountedMachineSize { calls: &y_calls, value: 2 };
        let z = CountedMachineSize { calls: &z_calls, value: 3 };
        let args = [CostArgument::from(&x), CostArgument::from(&y), CostArgument::from(&z)];

        BuiltinCosts::default().get_cost(DefaultFunction::WriteBits, args.as_slice());

        assert_eq!(x_calls.get(), 1);
        assert_eq!(y_calls.get(), 1);
        assert_eq!(z_calls.get(), 0);
    }

    #[test]
    fn blake2b_256_measures_its_argument_once() {
        let calls = Cell::new(0);
        let argument = CountedMachineSize { calls: &calls, value: 42 };
        let args = [CostArgument::from(&argument)];

        BuiltinCosts::default().get_cost(DefaultFunction::Blake2b_256, args.as_slice());

        // Memory is constant; CPU depends on X. The shared lazy argument must not be measured twice.
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn choose_data_does_not_measure_constant_cost_arguments() {
        let calls: [Cell<u8>; 6] = std::array::from_fn(|_| Cell::new(0));
        let arguments: [CountedMachineSize<'_>; 6] =
            std::array::from_fn(|index| CountedMachineSize { calls: &calls[index], value: index as i64 });
        let args = arguments.each_ref().map(CostArgument::from);

        BuiltinCosts::default().get_cost(DefaultFunction::ChooseData, args.as_slice());

        assert!(calls.iter().all(|calls| calls.get() == 0));
    }

    #[test]
    fn insert_coin_measures_only_its_fourth_argument() {
        let x_calls = Cell::new(0);
        let y_calls = Cell::new(0);
        let z_calls = Cell::new(0);
        let u_calls = Cell::new(0);
        let x = CountedMachineSize { calls: &x_calls, value: 1 };
        let y = CountedMachineSize { calls: &y_calls, value: 2 };
        let z = CountedMachineSize { calls: &z_calls, value: 3 };
        let u = CountedMachineSize { calls: &u_calls, value: 4 };
        let args = [CostArgument::from(&x), CostArgument::from(&y), CostArgument::from(&z), CostArgument::from(&u)];

        BuiltinCosts::default().get_cost(DefaultFunction::InsertCoin, args.as_slice());

        assert_eq!(x_calls.get(), 0);
        assert_eq!(y_calls.get(), 0);
        assert_eq!(z_calls.get(), 0);
        assert_eq!(u_calls.get(), 1);
    }
}
