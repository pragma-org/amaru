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

use std::{fs, time::Duration};

use amaru_kernel::{PROTOCOL_VERSION_10, PlutusVersion};
use amaru_uplc::{arena::Arena, binder::DeBruijn, flat};
use bumpalo::Bump;
use criterion::{Criterion, criterion_group, criterion_main};
use itertools::Itertools;

pub fn bench_plutus_use_cases(c: &mut Criterion) {
    let data_dir = std::path::Path::new("benches/use_cases/plutus_use_cases");

    for path in fs::read_dir(data_dir).unwrap().map(|entry| entry.unwrap()).map(|entry| entry.path()).sorted() {
        if path.is_file() {
            let file_name = path.file_name().unwrap().to_str().unwrap().replace(".flat", "");

            let script = std::fs::read(&path).unwrap();

            let mut arena = Arena::from_bump(Bump::with_capacity(1_048_576));

            c.bench_function(&file_name, |b| {
                b.iter(|| {
                    let program = flat::decode::<DeBruijn>(&arena, &script, PlutusVersion::V3, PROTOCOL_VERSION_10)
                        .expect("Failed to decode");

                    let result = program.eval_default(&arena);

                    let _term = result.term.expect("Failed to evaluate");

                    arena.reset();
                })
            });
        }
    }
}

criterion_group! {
    name = plutus_use_cases;
    config = Criterion::default()
        .measurement_time(Duration::from_secs(10));
    targets = bench_plutus_use_cases
}

criterion_main! {
    plutus_use_cases,
}
