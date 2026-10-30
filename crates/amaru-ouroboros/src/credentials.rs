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

//! In-process block-producer secrets for a testnet trial.
//!
//! The KES signing key, VRF signing key, and operational certificate are read from the
//! unencrypted text envelopes cardano-cli already writes. The cold key stays in
//! the certificate file; it is not loaded on its own.

use std::{fmt, path::Path, sync::Mutex};

use amaru_kernel::{
    Bytes, Ed25519Signature, Hash, Header, HeaderBody, HeaderHash, KesPeriod, KesSignature, NetworkName,
    OperationalCert, Point, ProtocolVersion, VerificationKey, VrfCert, VrfVerificationKey,
    cardano::fixed_bytes::FixedBytes,
    cbor, ed25519,
    size::BLOCK_BODY,
    utils::envelope::{HasEnvelope, ReadEnvelopeError, read_envelope},
};
use amaru_ouroboros_traits::{ChainStore, ForgingCredentials, ForgingCredentialsError, IssuerFields, StoreError};
use thiserror::Error;

use crate::{
    kes,
    praos::header::AssertOperationalCertificateError,
    vrf::{self, SecretKeyError as VrfKeyError},
};

const OPERATIONAL_CERTIFICATE_ENVELOPE: &str = "NodeOperationalCertificate";

/// Block-producer secrets loaded from cardano-cli files and held in this process.
///
/// The KES signing key evolves in memory and is not written back. A later
/// operational certificate is used only after the process starts again.
pub struct InProcessCredentials {
    inner: Mutex<KesState>,
    vrf: vrf::SecretKey,
    cold: VerificationKey,
    vrf_verification_key: VrfVerificationKey,
    operational_cert: OperationalCert,
    max_kes_evolutions: u64,
}

struct KesState {
    kes: kes::SecretKey,
    /// Absolute period the in-memory key can sign.
    period: KesPeriod,
}

impl fmt::Debug for InProcessCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InProcessCredentials")
            .field("pool_id", &self.pool_id())
            .field("sequence", &self.operational_cert.operational_cert_sequence_number)
            .field("kes_period", &self.operational_cert.operational_cert_kes_period)
            .finish_non_exhaustive()
    }
}

impl InProcessCredentials {
    /// Load the three cardano-cli envelopes and check that they belong together.
    ///
    /// The KES verification key must be the certificate's hot key, and the cold
    /// signature over that key, the sequence number, and the start period must
    /// verify. The sequence compared with the chain is checked separately, once
    /// the chain store is open. The certificate CBOR is decoded under the rules
    /// of `protocol_version`.
    pub fn from_files(
        kes_signing_key: impl AsRef<Path>,
        vrf_signing_key: impl AsRef<Path>,
        operational_certificate: impl AsRef<Path>,
        max_kes_evolutions: u64,
        protocol_version: ProtocolVersion,
    ) -> Result<Self, CredentialsError> {
        let mut kes = kes::SecretKey::from_file(kes_signing_key)?;
        let vrf = vrf::SecretKey::from_file(vrf_signing_key)?;
        let (certificate, cold) = load_operational_certificate(operational_certificate, protocol_version)?;
        let hot = kes::PublicKey::from(&mut kes);
        if *hot != *certificate.operational_cert_kes_verification_key {
            return Err(CredentialsError::HotKeyMismatch);
        }
        // The declared sequence compared with itself is always in range, so this
        // checks the cold signature and the cold key without needing the chain.
        let issuer = cold_verifying_key(&cold).ok_or(CredentialsError::ColdKey)?;
        AssertOperationalCertificateError::new(
            &certificate,
            &issuer,
            Some(certificate.operational_cert_sequence_number),
        )
        .map_err(|error| CredentialsError::Signature(error.to_string()))?;
        Ok(Self {
            inner: Mutex::new(KesState { kes, period: certificate.operational_cert_kes_period }),
            vrf_verification_key: VrfVerificationKey::from(*vrf::PublicKey::from(&vrf)),
            vrf,
            cold,
            operational_cert: certificate,
            max_kes_evolutions,
        })
    }
}

impl ForgingCredentials for InProcessCredentials {
    fn issuer_verification_key(&self) -> VerificationKey {
        self.cold
    }

    fn vrf_secret_bytes(&self) -> [u8; 32] {
        self.vrf.to_bytes()
    }

    fn issuer_fields(&self) -> IssuerFields {
        IssuerFields {
            issuer_verification_key: self.cold,
            vrf_verification_key: self.vrf_verification_key,
            operational_cert: self.operational_cert.clone(),
        }
    }

    fn sign(&self, period: KesPeriod, message: &[u8]) -> Result<KesSignature, ForgingCredentialsError> {
        let evolution =
            period.evolutions_since(self.operational_cert.operational_cert_kes_period, self.max_kes_evolutions)?;
        let mut inner = self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous = inner.period;
        inner.kes.evolve_to(evolution).map_err(|error| {
            if matches!(error, kes::KesError::CannotEvolveBackwards { .. }) {
                ForgingCredentialsError::EvolvedPast(error.to_string())
            } else {
                ForgingCredentialsError::Kes(error.to_string())
            }
        })?;
        let signature = KesSignature::from(<[u8; kes::Signature::SIZE]>::from(&inner.kes.sign(message)));
        if period != previous {
            inner.period = period;
            amaru_observability::info!(consensus::forge::KES_PERIOD, period = period);
        }
        Ok(signature)
    }
}

/// Load forging files when every path is set, and stay a follower when none are.
///
/// Mainnet refuses the files. Preprod, preview, and other testnets accept them.
/// Passing some of the paths and not the others fails.
pub fn forging_credentials_from_files(
    network: NetworkName,
    max_kes_evolutions: u64,
    kes_signing_key: Option<&Path>,
    vrf_signing_key: Option<&Path>,
    operational_certificate: Option<&Path>,
    protocol_version: ProtocolVersion,
) -> Result<Option<InProcessCredentials>, CredentialsError> {
    match (network, kes_signing_key, vrf_signing_key, operational_certificate) {
        (_, None, None, None) => Ok(None),
        (NetworkName::Mainnet, _, _, _) => Err(CredentialsError::Mainnet),
        (_, Some(kes), Some(vrf), Some(certificate)) => {
            Ok(Some(InProcessCredentials::from_files(kes, vrf, certificate, max_kes_evolutions, protocol_version)?))
        }
        _ => Err(CredentialsError::PartialFlags),
    }
}

/// Fail startup when the adopted chain would reject this operational certificate.
///
/// The cold signature must verify. The sequence number must be the counter stored
/// for this pool as of `tip`, or exactly one ahead. A pool with no counter yet may
/// use sequence 0 or 1. The header at `tip` counts: the counter is read from a
/// probe whose parent is that tip, so the tip's own certificate is included.
pub fn ensure_operational_certificate_accepted(
    credentials: &dyn ForgingCredentials,
    store: &dyn ChainStore,
    tip: &Point,
) -> Result<(), CertificateRejected> {
    let fields = credentials.issuer_fields();
    let issuer = cold_verifying_key(&fields.issuer_verification_key).ok_or(CertificateRejected::ColdKey)?;
    let latest = match tip {
        Point::Origin => None,
        Point::Specific(_, hash, _) => {
            let probe = header_parented_on(*hash);
            store.get_latest_opcert_sequence_number(&credentials.pool_id(), &probe)?
        }
    };
    AssertOperationalCertificateError::new(&fields.operational_cert, &issuer, latest)
        .map_err(|error| CertificateRejected::Rejected(Box::new(error)))
}

fn cold_verifying_key(cold: &VerificationKey) -> Option<ed25519::VerifyingKey> {
    ed25519::VerifyingKey::try_from(cold.as_slice()).ok()
}

/// Header whose parent is `parent`. The chain store reads the opcert counter as of that parent.
fn header_parented_on(parent: HeaderHash) -> Header {
    Header::new(
        HeaderBody {
            block_number: 0,
            slot: 0,
            prev_hash: Some(parent),
            issuer_verification_key: VerificationKey::zeroes(),
            vrf_verification_key: VrfVerificationKey::zeroes(),
            vrf_result: VrfCert { output: Bytes::default(), proof: FixedBytes::zeroes() },
            block_body_size: 0,
            block_body_hash: Hash::new([0u8; BLOCK_BODY]),
            operational_cert: OperationalCert {
                operational_cert_kes_verification_key: VerificationKey::zeroes(),
                operational_cert_sequence_number: 0,
                operational_cert_kes_period: KesPeriod::from(0),
                operational_cert_sigma: Ed25519Signature::zeroes(),
            },
            protocol_version: ProtocolVersion::new(11, 0),
        },
        KesSignature::zeroes(),
    )
}

/// Payload of a `NodeOperationalCertificate` envelope: the certificate followed by the
/// cold verification key that issued it.
struct OperationalCertificateFile {
    certificate: OperationalCert,
    cold: VerificationKey,
}

impl HasEnvelope for OperationalCertificateFile {
    const ACCEPTED_TYPES: &'static [&'static str] = &[OPERATIONAL_CERTIFICATE_ENVELOPE];
}

impl<'d, C: cbor::HasProtocolVersion> cbor::Decode<'d, C> for OperationalCertificateFile {
    fn decode(d: &mut cbor::Decoder<'d>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        match d.array()? {
            Some(2) => {}
            Some(len) => {
                return Err(cbor::decode::Error::message(format!(
                    "operational certificate CBOR must be a two-element array, found {len} elements"
                )));
            }
            None => {
                return Err(cbor::decode::Error::message(
                    "operational certificate CBOR must be a definite two-element array",
                ));
            }
        }
        Ok(Self { certificate: d.decode_with(ctx)?, cold: d.decode_with(ctx)? })
    }
}

/// Load an operational certificate and its cold verification key from a cardano-cli text
/// envelope file, decoding the CBOR payload under the rules of `protocol_version`.
pub fn load_operational_certificate(
    path: impl AsRef<Path>,
    protocol_version: ProtocolVersion,
) -> Result<(OperationalCert, VerificationKey), ReadEnvelopeError> {
    let mut ctx = protocol_version;
    let file: OperationalCertificateFile = read_envelope(path, &mut ctx)?;
    Ok((file.certificate, file.cold))
}

/// Why the forging files were refused before the node serves traffic.
#[derive(Debug, Error)]
pub enum CredentialsError {
    #[error("KES signing key: {0}")]
    Kes(#[from] kes::KesError),
    #[error("VRF signing key: {0}")]
    Vrf(#[from] VrfKeyError),
    #[error("operational certificate: {0}")]
    Certificate(#[from] ReadEnvelopeError),
    #[error("KES verification key does not match the operational certificate hot key")]
    HotKeyMismatch,
    #[error("operational certificate cold verification key is not a valid Ed25519 key")]
    ColdKey,
    #[error("operational certificate signature: {0}")]
    Signature(String),
    #[error(
        "block forging key files are refused on mainnet; they are available on preprod, preview, and other testnets"
    )]
    Mainnet,
    #[error("block forging needs --operator-kes, --operator-vrf, and --operator-cert together")]
    PartialFlags,
}

/// The operational certificate would not be accepted on the adopted chain.
#[derive(Debug, Error)]
pub enum CertificateRejected {
    #[error("could not read the operational certificate counter from the chain store: {0}")]
    Store(#[from] StoreError),
    #[error("operational certificate is not valid for the adopted chain: {0}")]
    Rejected(Box<AssertOperationalCertificateError>),
    #[error("operational certificate cold verification key is not a valid Ed25519 key")]
    ColdKey,
}

/// Payload of a `StakePoolVerificationKey_ed25519` envelope: the pool cold verification key.
struct ColdVerificationKeyFile(VerificationKey);

impl HasEnvelope for ColdVerificationKeyFile {
    const ACCEPTED_TYPES: &'static [&'static str] = &["StakePoolVerificationKey_ed25519"];
}

impl<'d, C: cbor::HasProtocolVersion> cbor::Decode<'d, C> for ColdVerificationKeyFile {
    fn decode(d: &mut cbor::Decoder<'d>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let cold: VerificationKey = d.decode_with(ctx)?;
        if cold_verifying_key(&cold).is_none() {
            return Err(cbor::decode::Error::message("cold verification key is not a valid Ed25519 key"));
        }
        Ok(Self(cold))
    }
}

/// Load a stake pool cold verification key from a cardano-cli text envelope file,
/// decoding the CBOR payload under the rules of `protocol_version`.
///
/// The file must have type `StakePoolVerificationKey_ed25519`. The key bytes are returned as a
/// [`VerificationKey`] suitable for pool ID derivation.
pub fn load_cold_verification_key(
    path: impl AsRef<Path>,
    protocol_version: ProtocolVersion,
) -> Result<VerificationKey, ReadEnvelopeError> {
    let mut ctx = protocol_version;
    Ok(read_envelope::<_, ColdVerificationKeyFile>(path, &mut ctx)?.0)
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use amaru_kernel::{
        IsHeader, Slot, cbor,
        ed25519::Signer,
        protocol_version::{PROTOCOL_VERSION_11, PROTOCOL_VERSION_12},
    };
    use amaru_ouroboros_traits::{HeaderDraft, InMemoryChainStore, WriteChainStore, kes_message};
    use serde::Deserialize;

    use super::*;
    use crate::praos::header::AssertKesSignatureError;

    const MAX_EVOLUTIONS: u64 = 62;

    fn envelope(r#type: &str, cbor_hex: &str) -> String {
        format!(r#"{{"type":"{type}","description":"","cborHex":"{cbor_hex}"}}"#)
    }

    /// cardano-cli writes the VRF seed followed by the verification key.
    fn vrf_signing_envelope(seed: &[u8; 32]) -> String {
        let secret = vrf::SecretKey::from(seed);
        let verification = vrf::PublicKey::from(&secret);
        let mut raw = [0u8; 64];
        raw[..32].copy_from_slice(seed);
        raw[32..].copy_from_slice(verification.as_ref());
        envelope("VrfSigningKey_PraosVRF", &format!("5840{}", hex::encode(raw)))
    }

    fn kes_envelope() -> String {
        // The on-disk envelope is the 608-byte seed. The in-memory key appends the period.
        let key = kes::SecretKey::for_tests();
        let raw = unsafe { key.leak_into_bytes() };
        envelope("KesSigningKey_ed25519_kes_2^6", &format!("590260{}", hex::encode(&raw[..kes::SecretKey::SIZE])))
    }

    struct Issued {
        certificate: OperationalCert,
        cold_vk: VerificationKey,
    }

    fn issue(sequence: u64, start: KesPeriod) -> Issued {
        let cold = ed25519::SigningKey::from_bytes(&[9u8; 32]);
        let mut kes = kes::SecretKey::for_tests();
        let hot = kes::PublicKey::from(&mut kes);
        let mut message = Vec::with_capacity(48);
        message.extend_from_slice(&hot[..]);
        message.extend_from_slice(&sequence.to_be_bytes());
        message.extend_from_slice(&u64::from(start).to_be_bytes());
        let certificate = OperationalCert {
            operational_cert_kes_verification_key: FixedBytes::from(*hot),
            operational_cert_sequence_number: sequence,
            operational_cert_kes_period: start,
            operational_cert_sigma: Ed25519Signature::from(cold.sign(&message).to_bytes()),
        };
        let cold_vk = VerificationKey::from(cold.verifying_key().to_bytes());
        Issued { certificate, cold_vk }
    }

    fn corrupt_signature(certificate: &mut OperationalCert) {
        let mut sigma = *certificate.operational_cert_sigma.as_array();
        sigma[0] ^= 0xff;
        certificate.operational_cert_sigma = Ed25519Signature::from(sigma);
    }

    fn certificate_cbor(certificate: &OperationalCert, cold: &VerificationKey) -> String {
        let mut buf = Vec::new();
        let mut encoder = cbor::Encoder::new(&mut buf);
        let mut ctx = PROTOCOL_VERSION_12;
        encoder.array(2).unwrap();
        encoder.encode_with(certificate, &mut ctx).unwrap();
        encoder.encode_with(cold, &mut ctx).unwrap();
        hex::encode(buf)
    }

    fn write_files(dir: &Path, issued: &Issued) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
        let kes_path = dir.join("kes.skey");
        let vrf_path = dir.join("vrf.skey");
        let cert_path = dir.join("node.cert");
        fs::write(&kes_path, kes_envelope()).unwrap();
        fs::write(&vrf_path, vrf_signing_envelope(&[7u8; 32])).unwrap();
        fs::write(
            &cert_path,
            envelope(OPERATIONAL_CERTIFICATE_ENVELOPE, &certificate_cbor(&issued.certificate, &issued.cold_vk)),
        )
        .unwrap();
        (kes_path, vrf_path, cert_path)
    }

    fn load(issued: &Issued) -> InProcessCredentials {
        let dir = tempfile::tempdir().unwrap();
        let (kes_path, vrf_path, cert_path) = write_files(dir.path(), issued);
        InProcessCredentials::from_files(&kes_path, &vrf_path, &cert_path, MAX_EVOLUTIONS, PROTOCOL_VERSION_12).unwrap()
    }

    fn draft() -> HeaderDraft {
        HeaderDraft {
            block_number: 1,
            slot: Slot::from(1),
            prev_hash: None,
            vrf_result: VrfCert { output: Bytes::default(), proof: FixedBytes::zeroes() },
            block_body_size: 0,
            block_body_hash: Hash::new([0u8; BLOCK_BODY]),
            protocol_version: ProtocolVersion::new(11, 0),
        }
    }

    fn signed(credentials: &InProcessCredentials, period: KesPeriod) -> Header {
        let body = draft().body(&credentials.issuer_fields());
        let signature = credentials.sign(period, &kes_message(&body)).unwrap();
        Header::new(body, signature)
    }

    fn assert_signature(credentials: &InProcessCredentials, period: KesPeriod, header: &Header) {
        let ocert = &header.body().operational_cert;
        let hot = kes::PublicKey::from(ocert.operational_cert_kes_verification_key.as_array());
        let signature = kes::Signature::from(header.signature().as_array());
        AssertKesSignatureError::new(
            period,
            ocert.operational_cert_kes_period,
            header.body(),
            &hot,
            &signature,
            MAX_EVOLUTIONS,
        )
        .unwrap();
        assert_eq!(credentials.issuer_fields().operational_cert, *ocert);
    }

    #[test]
    fn loaded_envelopes_sign_and_match_the_certificate() {
        let issued = issue(0, KesPeriod::from(4));
        let credentials = load(&issued);
        assert_eq!(credentials.issuer_verification_key(), issued.cold_vk);
        assert_eq!(credentials.vrf_secret_bytes(), [7u8; 32]);
        let header = signed(&credentials, KesPeriod::from(4));
        assert_signature(&credentials, KesPeriod::from(4), &header);
        let later = signed(&credentials, KesPeriod::from(5));
        assert_signature(&credentials, KesPeriod::from(5), &later);
    }

    fn forging_fixture(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/forging").join(name)
    }

    fn read_envelope(path: &Path) -> (String, Vec<u8>) {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Envelope {
            r#type: String,
            cbor_hex: String,
        }
        let envelope: Envelope = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        (envelope.r#type, hex::decode(envelope.cbor_hex).unwrap())
    }

    fn cbor_byte_string(path: &Path) -> Vec<u8> {
        let (_type, payload) = read_envelope(path);
        let mut decoder = cbor::Decoder::new(&payload);
        cbor::decode_bytes(&mut decoder).unwrap().into_owned()
    }

    /// Throwaway keys from `cardano-cli conway node` (`key-gen-VRF`, `key-gen-KES`, `key-gen`,
    /// `issue-op-cert`). Not a live stake pool.
    #[test]
    fn cardano_cli_conway_fixtures_load_and_verify() {
        let kes_path = forging_fixture("kes.skey");
        let vrf_path = forging_fixture("vrf.skey");
        let cert_path = forging_fixture("node.cert");
        let kes_vk = cbor_byte_string(&forging_fixture("kes.vkey"));
        let vrf_vk = cbor_byte_string(&forging_fixture("vrf.vkey"));
        let cold_vk = cbor_byte_string(&forging_fixture("cold.vkey"));

        let mut kes = kes::SecretKey::from_file(&kes_path).unwrap();
        assert_eq!(u32::from(kes.period()), 0, "a fresh cardano-cli KES file has no period");
        assert_eq!(kes::PublicKey::from(&mut kes).as_ref(), kes_vk.as_slice());

        let vrf = vrf::SecretKey::from_file(&vrf_path).unwrap();
        assert_eq!(vrf::PublicKey::from(&vrf).as_ref(), vrf_vk.as_slice());
        let input = vrf::Input::from(&[11u8; vrf::Input::SIZE]);
        let proof = vrf.prove(&input);
        proof.verify(&vrf::PublicKey::from(&vrf), &input).unwrap();

        let credentials =
            InProcessCredentials::from_files(&kes_path, &vrf_path, &cert_path, MAX_EVOLUTIONS, PROTOCOL_VERSION_12)
                .unwrap();
        assert_eq!(credentials.issuer_verification_key().as_slice(), cold_vk.as_slice());
        let fields = credentials.issuer_fields();
        assert_eq!(fields.operational_cert.operational_cert_kes_verification_key.as_slice(), kes_vk.as_slice());
        assert_eq!(fields.vrf_verification_key.as_slice(), vrf_vk.as_slice());
        assert_eq!(fields.operational_cert.operational_cert_sequence_number, 0);
        assert_eq!(u64::from(fields.operational_cert.operational_cert_kes_period), 7);
        let header = signed(&credentials, KesPeriod::from(7));
        assert_signature(&credentials, KesPeriod::from(7), &header);

        let (counter_type, counter_cbor) = read_envelope(&forging_fixture("cold.counter"));
        assert_eq!(counter_type, "NodeOperationalCertificateIssueCounter");
        let mut decoder = cbor::Decoder::new(&counter_cbor);
        assert_eq!(decoder.array().unwrap(), Some(2));
        assert_eq!(decoder.u64().unwrap(), 1, "issue-op-cert rewrites the counter to the next number");
        assert_eq!(cbor::decode_bytes(&mut decoder).unwrap().as_ref(), cold_vk.as_slice());

        let (cold_type, cold_skey) = read_envelope(&forging_fixture("cold.skey"));
        assert_eq!(cold_type, "StakePoolSigningKey_ed25519");
        let mut decoder = cbor::Decoder::new(&cold_skey);
        assert_eq!(cbor::decode_bytes(&mut decoder).unwrap().len(), 32);
    }

    #[test]
    fn period_outside_the_window_does_not_evolve_the_key() {
        let credentials = load(&issue(0, KesPeriod::from(4)));
        let before = credentials.sign(KesPeriod::from(3), b"too early");
        assert!(matches!(before, Err(ForgingCredentialsError::Period(_))), "{before:?}");
        let expired = credentials.sign(KesPeriod::from(4 + MAX_EVOLUTIONS), b"expired");
        assert!(matches!(expired, Err(ForgingCredentialsError::Period(_))), "{expired:?}");
        let header = signed(&credentials, KesPeriod::from(4));
        assert_signature(&credentials, KesPeriod::from(4), &header);
    }

    #[test]
    fn signing_an_earlier_period_fails_after_the_key_has_evolved() {
        let credentials = load(&issue(0, KesPeriod::from(4)));
        let _ = signed(&credentials, KesPeriod::from(5));
        let err = credentials.sign(KesPeriod::from(4), b"behind").unwrap_err();
        assert!(matches!(err, ForgingCredentialsError::EvolvedPast(_)), "{err:?}");
    }

    #[test]
    fn hot_key_mismatch_fails_at_load() {
        let mut issued = issue(0, KesPeriod::from(0));
        issued.certificate.operational_cert_kes_verification_key = FixedBytes::zeroes();
        let dir = tempfile::tempdir().unwrap();
        let (kes_path, vrf_path, cert_path) = write_files(dir.path(), &issued);
        let err =
            InProcessCredentials::from_files(&kes_path, &vrf_path, &cert_path, MAX_EVOLUTIONS, PROTOCOL_VERSION_12)
                .unwrap_err();
        assert!(matches!(err, CredentialsError::HotKeyMismatch), "{err}");
    }

    #[test]
    fn bad_cold_signature_fails_at_load() {
        let mut issued = issue(1, KesPeriod::from(0));
        corrupt_signature(&mut issued.certificate);
        let dir = tempfile::tempdir().unwrap();
        let (kes_path, vrf_path, cert_path) = write_files(dir.path(), &issued);
        let err =
            InProcessCredentials::from_files(&kes_path, &vrf_path, &cert_path, MAX_EVOLUTIONS, PROTOCOL_VERSION_12)
                .unwrap_err();
        assert!(matches!(err, CredentialsError::Signature(_)), "{err}");
    }

    #[test]
    fn wrong_envelope_type_and_truncated_cbor_fail() {
        let issued = issue(0, KesPeriod::from(0));
        let wrong =
            envelope("NodeOperationalCertificateIssue", &certificate_cbor(&issued.certificate, &issued.cold_vk));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("node.cert");
        fs::write(&path, wrong).unwrap();
        let err = load_operational_certificate(&path, PROTOCOL_VERSION_12).unwrap_err();
        assert!(matches!(err, ReadEnvelopeError::UnexpectedType { .. }), "{err}");

        let truncated = envelope(OPERATIONAL_CERTIFICATE_ENVELOPE, "8200");
        fs::write(&path, truncated).unwrap();
        let err = load_operational_certificate(&path, PROTOCOL_VERSION_12).unwrap_err();
        assert!(matches!(err, ReadEnvelopeError::Cbor(_)), "{err}");
    }

    #[test]
    fn mainnet_and_partial_flags_fail_and_no_flags_stay_a_follower() {
        let kes = Path::new("kes.skey");
        let vrf = Path::new("vrf.skey");
        let cert = Path::new("node.cert");
        let mainnet = forging_credentials_from_files(
            NetworkName::Mainnet,
            MAX_EVOLUTIONS,
            Some(kes),
            Some(vrf),
            Some(cert),
            PROTOCOL_VERSION_12,
        );
        assert!(matches!(mainnet, Err(CredentialsError::Mainnet)), "{mainnet:?}");
        let partial = forging_credentials_from_files(
            NetworkName::Preprod,
            MAX_EVOLUTIONS,
            Some(kes),
            None,
            None,
            PROTOCOL_VERSION_12,
        );
        assert!(matches!(partial, Err(CredentialsError::PartialFlags)), "{partial:?}");
        assert!(
            forging_credentials_from_files(NetworkName::Mainnet, MAX_EVOLUTIONS, None, None, None, PROTOCOL_VERSION_12)
                .unwrap()
                .is_none()
        );
        assert!(
            forging_credentials_from_files(NetworkName::Preview, MAX_EVOLUTIONS, None, None, None, PROTOCOL_VERSION_12)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn preprod_with_the_three_files_loads_credentials() {
        let issued = issue(2, KesPeriod::from(1));
        let dir = tempfile::tempdir().unwrap();
        let (kes_path, vrf_path, cert_path) = write_files(dir.path(), &issued);
        let loaded = forging_credentials_from_files(
            NetworkName::Preprod,
            MAX_EVOLUTIONS,
            Some(&kes_path),
            Some(&vrf_path),
            Some(&cert_path),
            PROTOCOL_VERSION_12,
        )
        .unwrap()
        .expect("preprod accepts the three files");
        assert_eq!(loaded.issuer_verification_key(), issued.cold_vk);
        assert_eq!(loaded.issuer_fields().operational_cert.operational_cert_sequence_number, 2);
    }

    fn store_producer_header(store: &InMemoryChainStore, credentials: &InProcessCredentials, slot: u64) -> Point {
        let issuer = credentials.issuer_fields();
        let header = Header::new(
            HeaderBody {
                block_number: 1,
                slot,
                prev_hash: None,
                issuer_verification_key: issuer.issuer_verification_key,
                vrf_verification_key: issuer.vrf_verification_key,
                vrf_result: VrfCert { output: Bytes::default(), proof: FixedBytes::zeroes() },
                block_body_size: 0,
                block_body_hash: Hash::new([0u8; BLOCK_BODY]),
                operational_cert: issuer.operational_cert,
                protocol_version: ProtocolVersion::new(11, 0),
            },
            KesSignature::zeroes(),
        );
        store.store_header(&header).unwrap();
        Point::Specific(header.slot(), header.hash(), header.block_height())
    }

    #[test]
    fn startup_accepts_the_stored_counter_or_one_ahead() {
        let on_chain = load(&issue(4, KesPeriod::from(0)));
        let store = InMemoryChainStore::new();
        let tip = store_producer_header(&store, &on_chain, 100);
        ensure_operational_certificate_accepted(&on_chain, &store, &tip).unwrap();

        let next = load(&issue(5, KesPeriod::from(0)));
        ensure_operational_certificate_accepted(&next, &store, &tip).unwrap();

        let ahead = load(&issue(6, KesPeriod::from(0)));
        let err = ensure_operational_certificate_accepted(&ahead, &store, &tip).unwrap_err();
        assert!(err.to_string().contains("too far ahead"), "{err}");

        let behind = load(&issue(3, KesPeriod::from(0)));
        let err = ensure_operational_certificate_accepted(&behind, &store, &tip).unwrap_err();
        assert!(err.to_string().contains("less than"), "{err}");
    }

    #[test]
    fn startup_on_an_empty_chain_accepts_the_first_certificate_only() {
        let store = InMemoryChainStore::new();
        ensure_operational_certificate_accepted(&load(&issue(0, KesPeriod::from(0))), &store, &Point::Origin).unwrap();
        ensure_operational_certificate_accepted(&load(&issue(1, KesPeriod::from(0))), &store, &Point::Origin).unwrap();
        let err = ensure_operational_certificate_accepted(&load(&issue(5, KesPeriod::from(0))), &store, &Point::Origin)
            .unwrap_err();
        assert!(err.to_string().contains("too far ahead"), "{err}");
    }

    #[test]
    fn startup_reports_a_cold_signature_the_chain_would_reject() {
        let mut issued = issue(0, KesPeriod::from(0));
        corrupt_signature(&mut issued.certificate);
        let credentials = ForgedIdentity { issuer: issuer_from(&issued) };
        let err = ensure_operational_certificate_accepted(&credentials, &InMemoryChainStore::new(), &Point::Origin)
            .unwrap_err();
        assert!(err.to_string().contains("Invalid operational certificate signature"), "{err}");
    }

    fn issuer_from(issued: &Issued) -> IssuerFields {
        IssuerFields {
            issuer_verification_key: issued.cold_vk,
            vrf_verification_key: VrfVerificationKey::from(*vrf::PublicKey::from(&vrf::SecretKey::from(&[7u8; 32]))),
            operational_cert: issued.certificate.clone(),
        }
    }

    struct ForgedIdentity {
        issuer: IssuerFields,
    }

    impl ForgingCredentials for ForgedIdentity {
        fn issuer_verification_key(&self) -> VerificationKey {
            self.issuer.issuer_verification_key
        }

        fn vrf_secret_bytes(&self) -> [u8; 32] {
            [0u8; 32]
        }

        fn issuer_fields(&self) -> IssuerFields {
            self.issuer.clone()
        }

        fn sign(&self, _period: KesPeriod, _message: &[u8]) -> Result<KesSignature, ForgingCredentialsError> {
            Ok(KesSignature::zeroes())
        }
    }

    #[test]
    fn load_cold_verification_key_from_fixture() {
        let cold_vkey_path = forging_fixture("cold.vkey");
        let cold =
            load_cold_verification_key(&cold_vkey_path, PROTOCOL_VERSION_12).expect("cold.vkey fixture should load");
        let credentials = InProcessCredentials::from_files(
            forging_fixture("kes.skey"),
            forging_fixture("vrf.skey"),
            forging_fixture("node.cert"),
            MAX_EVOLUTIONS,
            PROTOCOL_VERSION_12,
        )
        .unwrap();
        assert_eq!(cold, credentials.issuer_verification_key(), "cold.vkey must match the key embedded in node.cert");
    }

    #[test]
    fn load_cold_verification_key_decodes_under_the_given_protocol_version() {
        let issued = issue(0, KesPeriod::from(0));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cold.vkey");
        let chunked = format!("5f5820{}ff", hex::encode(&issued.cold_vk[..]));
        fs::write(&path, envelope("StakePoolVerificationKey_ed25519", &chunked)).unwrap();
        let cold = load_cold_verification_key(&path, PROTOCOL_VERSION_12).unwrap();
        assert_eq!(cold, issued.cold_vk);
        let err = load_cold_verification_key(&path, PROTOCOL_VERSION_11).unwrap_err();
        assert!(matches!(err, ReadEnvelopeError::Cbor(_)), "{err}");
    }

    #[test]
    fn load_cold_verification_key_rejects_wrong_envelope_type() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.vkey");
        fs::write(&path, envelope("SomeOtherType", "5820aabbcc")).unwrap();
        let err = load_cold_verification_key(&path, PROTOCOL_VERSION_12).unwrap_err();
        assert!(matches!(err, ReadEnvelopeError::UnexpectedType { .. }), "{err}");
    }

    #[test]
    fn load_cold_verification_key_rejects_an_invalid_ed25519_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.vkey");
        // y = 2 is not on the Ed25519 curve, so the 32 bytes cannot decompress to a point.
        fs::write(&path, envelope("StakePoolVerificationKey_ed25519", &format!("582002{}", "00".repeat(31)))).unwrap();
        let err = load_cold_verification_key(&path, PROTOCOL_VERSION_12).unwrap_err();
        assert!(err.to_string().contains("not a valid Ed25519 key"), "{err}");
    }

    #[test]
    fn load_operational_certificate_is_public_and_matches_fixture() {
        let cert_path = forging_fixture("node.cert");
        let (cert, cold) =
            load_operational_certificate(&cert_path, PROTOCOL_VERSION_12).expect("node.cert fixture should load");
        assert_eq!(cert.operational_cert_sequence_number, 0);
        assert_eq!(u64::from(cert.operational_cert_kes_period), 7);
        let credentials = InProcessCredentials::from_files(
            forging_fixture("kes.skey"),
            forging_fixture("vrf.skey"),
            &cert_path,
            MAX_EVOLUTIONS,
            PROTOCOL_VERSION_12,
        )
        .unwrap();
        assert_eq!(cold, credentials.issuer_verification_key());
    }
}
