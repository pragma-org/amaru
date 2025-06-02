// Copyright 2024 PRAGMA
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

use std::{
    env,
    fs::{read_dir, File},
    io::Write,
    path::Path,
};

use amaru_kernel::network::NetworkName;

fn network_name_to_string(network: &NetworkName) -> String {
    match network {
        NetworkName::Mainnet => "Mainnet".to_string(),
        NetworkName::Preprod => "Preprod".to_string(),
        NetworkName::Preview => "Preview".to_string(),
        NetworkName::Testnet(magic) => format!("Testnet:{}", magic),
    }
}

fn generated_compare_snapshot_test_cases() {
    let out_dir = env::var("OUT_DIR").unwrap();
    let dest_path = Path::new(&out_dir).join("generated_compare_snapshot_test_cases.rs");

    let snapshots_root = Path::new("tests/snapshots");
    let mut test_cases: Vec<(String, u64)> = Vec::new();

    for network_entry in read_dir(snapshots_root).unwrap() {
        let network_entry = network_entry.unwrap();
        let network_path = network_entry.path();

        if network_path.is_dir() {
            let network_name = network_path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned();

            for file_entry in read_dir(&network_path).unwrap() {
                let file_entry = file_entry.unwrap();
                let file_name = file_entry.file_name().to_string_lossy().into_owned();

                if let Some(epoch_str) = file_name
                    .strip_prefix("summary__rewards_summary_")
                    .and_then(|s| s.strip_suffix(".snap"))
                {
                    if let Ok(epoch) = epoch_str.parse::<u64>() {
                        test_cases.push((network_name.clone(), epoch));
                    }
                }
            }
        }
    }

    test_cases.sort();

    let mut file = File::create(&dest_path).unwrap();
    for (network, epoch) in test_cases {
        let network_name: NetworkName = network.parse().unwrap();
        writeln!(file, "#[test_case(NetworkName::{}, {})]", network_name_to_string(&network_name), epoch).unwrap();
    }

    let test_content = "
    #[ignore]
    pub fn compare_snapshot_test_case(network_name: NetworkName, epoch: u64) {
        let epoch = Epoch::from(epoch);
        compare_snapshot(network_name, epoch)
    }";
    writeln!(file, "{}", test_content).unwrap();
}

#[allow(clippy::expect_used)]
fn main() {
    built::write_built_file().expect("Failed to acquire build-time information");

    generated_compare_snapshot_test_cases();
}
