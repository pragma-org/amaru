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

use std::{fmt::Write, net::Ipv6Addr};

pub use pallas_primitives::conway::Relay;

use crate::Nullable;

pub fn fmt(relays: &[Relay]) -> String {
    let mut out = String::new();

    for (i, relay) in relays.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }

        match relay {
            Relay::MultiHostName(dns) => {
                out.push_str(dns.as_str());
            }

            Relay::SingleHostName(port, dns) => {
                out.push_str(dns.as_str());

                if let Nullable::Some(port) = port {
                    #[allow(clippy::expect_used)]
                    write!(out, ":{}", port).expect("writing to a String");
                }
            }

            Relay::SingleHostAddr(port, ipv4, ipv6) => {
                if let Nullable::Some(ipv4) = ipv4 {
                    #[allow(clippy::expect_used)]
                    write!(
                        out,
                        "{}.{}.{}.{}{}",
                        ipv4[0],
                        ipv4[1],
                        ipv4[2],
                        ipv4[3],
                        if let Nullable::Some(port) = port { format!(":{port}") } else { String::new() }
                    )
                    .expect("writing to a String");
                }

                if let Nullable::Some(ipv6) = ipv6 {
                    if matches!(ipv4, Nullable::Some(..)) {
                        out.push('|');
                    }

                    #[allow(clippy::expect_used)]
                    write!(
                        out,
                        "{}{}",
                        Ipv6Addr::from([
                            ipv6[3], ipv6[2], ipv6[1], ipv6[0], // group 1
                            ipv6[7], ipv6[6], ipv6[5], ipv6[4], // group 2
                            ipv6[11], ipv6[10], ipv6[9], ipv6[8], // group 3
                            ipv6[15], ipv6[14], ipv6[13], ipv6[12], // group 4
                        ]),
                        if let Nullable::Some(port) = port { format!(":{port}") } else { String::new() }
                    )
                    .expect("writing to a String");
                }
            }
        }
    }

    out
}
