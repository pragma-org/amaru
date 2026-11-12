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

use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    ops::Range,
    path::{Path, PathBuf},
};

use amaru::lifecycle::{Runnable, RuntimeKind};
use amaru_kernel::{
    ProtocolVersion, Transaction, VerificationKeyWitness,
    cbor::{self, WithOriginalBytes},
};
use amaru_ouroboros::credentials::TransactionSigningKey;
use anyhow::Context;
use tempfile::NamedTempFile;

#[derive(Debug, clap::Subcommand)]
pub(crate) enum TransactionCommand {
    Sign(SignArgs),
}

impl TransactionCommand {
    pub fn into_runnable(self) -> Runnable {
        match self {
            Self::Sign(args) => args.into_runnable(),
        }
    }
}

/// Sign a Conway transaction offline, preserving its body and existing witnesses.
///
/// Adds key witnesses without building, balancing, validating against chain state, or submitting
/// the transaction. Extended and Byron signing keys are not supported.
#[derive(Debug, clap::Args)]
pub struct SignArgs {
    /// Unsigned or partially signed transaction, as binary CBOR.
    #[arg(long, value_name = amaru::value_names::FILEPATH)]
    transaction: PathBuf,

    /// Payment, stake, or pool cold key, as a raw 32-byte Ed25519 seed.
    /// Repeat this option for each signer.
    #[arg(long, required = true, value_name = amaru::value_names::FILEPATH)]
    signing_key: Vec<PathBuf>,

    /// Destination for the signed binary CBOR transaction. Existing files are never replaced.
    #[arg(long, value_name = amaru::value_names::FILEPATH)]
    output: PathBuf,
}

impl SignArgs {
    pub fn into_runnable(self) -> Runnable {
        Runnable::exit_on_signal(RuntimeKind::Simple, move || async move { self.run() })
    }

    pub fn run(self) -> anyhow::Result<()> {
        let mut ctx = ProtocolVersion::default();
        let bytes = fs::read(&self.transaction)
            .with_context(|| format!("could not read transaction {}", self.transaction.display()))?;
        let transaction = cbor::from_cbor_no_leftovers_with(&bytes, &mut ctx)
            .with_context(|| format!("could not decode transaction {}", self.transaction.display()))?;
        let keys = self
            .signing_key
            .iter()
            .map(|path| {
                TransactionSigningKey::from_file(path)
                    .with_context(|| format!("could not read signing key {}", path.display()))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let signed = sign_transaction(transaction, &keys, &mut ctx)?;
        let parent = self.output.parent().filter(|path| !path.as_os_str().is_empty()).unwrap_or(Path::new("."));
        let mut file = NamedTempFile::new_in(parent)
            .with_context(|| format!("could not create temporary file for {}", self.output.display()))?;
        file.write_all(&signed).context("could not write signed transaction")?;
        file.as_file().sync_all().context("could not flush signed transaction")?;
        file.persist_noclobber(&self.output)
            .with_context(|| format!("could not save signed transaction {}", self.output.display()))?;
        Ok(())
    }
}

/// Replace only the verification-key witness field; all other transaction payloads retain their bytes.
fn sign_transaction(
    transaction: WithOriginalBytes<Transaction>,
    keys: &[TransactionSigningKey],
    ctx: &mut ProtocolVersion,
) -> anyhow::Result<Vec<u8>> {
    anyhow::ensure!(!keys.is_empty(), "at least one signing key is required");
    let witnesses = transaction
        .witnesses
        .verification_key_witness
        .iter()
        .flat_map(|witnesses| witnesses.iter().cloned())
        .chain(keys.iter().map(|key| key.witness(transaction.tx_id())))
        .map(|witness| (witness.verification_key, witness))
        .collect::<BTreeMap<_, _>>()
        .into_values()
        .collect::<Vec<VerificationKeyWitness>>();

    let bytes = cbor::to_cbor_with(&transaction, ctx);
    let (witness_range, fields) = read_witness_fields(&bytes)?;
    let fields = replace_key_witnesses(fields, witnesses, ctx)?;
    encode_transaction(&bytes, witness_range, fields)
}

fn read_witness_fields(bytes: &[u8]) -> anyhow::Result<(Range<usize>, BTreeMap<u64, Vec<u8>>)> {
    let mut decoder = cbor::Decoder::new(bytes);
    decoder.array()?;
    decoder.skip()?;
    let start = decoder.position();
    let fields = cbor::heterogeneous_map_unique_keys(
        &mut decoder,
        BTreeMap::new(),
        |d| d.u64(),
        |d, fields, key| {
            let (_, bytes) = cbor::tee(d, |d| d.skip())?;
            fields.insert(key, bytes.to_vec());
            Ok(())
        },
    )?;
    Ok((start..decoder.position(), fields))
}

fn replace_key_witnesses(
    mut fields: BTreeMap<u64, Vec<u8>>,
    witnesses: Vec<VerificationKeyWitness>,
    ctx: &mut ProtocolVersion,
) -> anyhow::Result<BTreeMap<u64, Vec<u8>>> {
    let mut encoded_witnesses = cbor::Encoder::new(Vec::new());
    encoded_witnesses.tag(cbor::TAG_SET_258)?.encode_with(witnesses, ctx)?;
    fields.insert(0, encoded_witnesses.into_writer());
    Ok(fields)
}

fn encode_transaction(
    bytes: &[u8],
    witness_range: Range<usize>,
    fields: BTreeMap<u64, Vec<u8>>,
) -> anyhow::Result<Vec<u8>> {
    let mut output = cbor::Encoder::new(bytes[..witness_range.start].to_vec());
    output.map(fields.len() as u64)?;
    for (key, value) in fields {
        output.u64(key)?.writer_mut().extend_from_slice(&value);
    }
    output.writer_mut().extend_from_slice(&bytes[witness_range.end..]);
    Ok(output.into_writer())
}
