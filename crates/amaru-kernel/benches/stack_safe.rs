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

use amaru_kernel::{Metadatum, from_cbor, to_cbor, utils::stack};

const MAX_DEPTH: usize = 16_384;
const _512KIB: usize = 524_288;
const _20MIB: usize = 20_971_520;

pub fn main() {
    divan::main();
}

#[divan::bench(args = [_512KIB, _20MIB])]
fn nested_metadatum_cbor_decode(bencher: divan::Bencher<'_, '_>, stack_size: usize) {
    bencher
        .with_inputs(|| {
            let mut metadatum = Metadatum::bytes(vec![]);
            let mut depth = MAX_DEPTH;
            while depth > 1 {
                metadatum = Metadatum::array(vec![metadatum]);
                depth -= 1;
            }
            to_cbor(&metadatum)
        })
        .bench_local_values(|bytes| {
            stack::with_stack_size(stack_size, move || divan::black_box(from_cbor::<Metadatum>(&bytes).is_some()))
        })
}

#[divan::bench(args = [_512KIB, _20MIB])]
fn nested_metadatum_clone(bencher: divan::Bencher<'_, '_>, stack_size: usize) {
    bencher
        .with_inputs(|| {
            let mut metadatum = Metadatum::bytes(vec![]);
            let mut depth = MAX_DEPTH;
            while depth > 1 {
                metadatum = Metadatum::array(vec![metadatum]);
                depth -= 1;
            }
            metadatum
        })
        .bench_local_values(|data| stack::with_stack_size(stack_size, move || divan::black_box(data.clone())))
}
