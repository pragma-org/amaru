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

use std::str::FromStr;

use amaru::{
    lifecycle::{Runnable, RuntimeKind},
    version,
};
use amaru_kernel::{AmaruTag, AmaruTagError, ProtocolVersion, SoftwareTag};
use clap::Parser;

#[derive(Debug, Parser)]
pub struct Args {
    /// A protocol version (`11.4374593`), a minor component (`4374593`), or a minor in hex (`0x0042c041`).
    #[arg(value_name = "VERSION")]
    version: Minor,
}

impl Args {
    pub(crate) fn current() -> Self {
        Self { version: Minor(u32::from(version::software_tag())) }
    }
}

#[derive(Debug, Clone, Copy)]
struct Minor(u32);

impl FromStr for Minor {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
            return u32::from_str_radix(hex, 16).map(Self).map_err(|e| e.to_string());
        }
        if let Ok(minor) = s.parse::<u32>() {
            return Ok(Self(minor));
        }
        let version = s.parse::<ProtocolVersion>()?;
        u32::try_from(version.minor())
            .map(Self)
            .map_err(|_| format!("minor {} does not fit in 32 bits", version.minor()))
    }
}

pub(crate) fn runnable(args: Args) -> Runnable {
    Runnable::exit_on_signal(RuntimeKind::Simple, move || run(args))
}

#[expect(clippy::print_stdout)]
async fn run(args: Args) -> anyhow::Result<()> {
    let minor = args.version.0;
    let tag = SoftwareTag::try_from(minor)?;
    println!("minor: {minor} (0x{minor:08x})");
    println!("tag:   {tag}");
    match AmaruTag::try_from(tag) {
        Ok(amaru) => println!("amaru: {amaru}"),
        Err(AmaruTagError::Id(_) | AmaruTagError::NoSignal | AmaruTagError::OptOut) => {}
        Err(error) => println!("amaru: invalid, {error}"),
    }
    Ok(())
}
