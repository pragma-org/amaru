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

//! Forging credentials whose KES signing key is held by a separate signer executable.

use std::{
    io::{self, BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::{
        Mutex,
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
    },
    thread,
    time::{Duration, Instant},
};

use amaru_kernel::{KesPeriod, KesSignature, NetworkName, VerificationKey};
use amaru_ouroboros_traits::{ForgingCredentials, ForgingCredentialsError, IssuerFields};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    credentials::{CredentialsError, cold_verifying_key, load_operational_certificate},
    kes,
    praos::header::AssertOperationalCertificateError,
    vrf,
};

const RESTART_INTERVAL: Duration = Duration::from_secs(1);
const SUPERVISOR_POLL_INTERVAL: Duration = Duration::from_millis(200);
const SIGNER_STARTUP_TIMEOUT: Duration = Duration::from_secs(5);
const SIGN_RESPONSE_TIMEOUT: Duration = Duration::from_secs(2);

/// The node's public and VRF credentials, with KES signing delegated to a child.
///
/// The KES path is passed to the child unopened. No KES secret is decoded in this process.
pub struct ProcessCredentials {
    vrf: vrf::SecretKey,
    issuer: IssuerFields,
    max_kes_evolutions: u64,
    signer: Sender<SignRequest>,
    last_logged_period: Mutex<KesPeriod>,
}

impl ProcessCredentials {
    /// Load the VRF key and certificate, then start and verify the KES signer beside this executable.
    pub fn from_files(
        kes_path: impl AsRef<Path>,
        vrf_path: impl AsRef<Path>,
        certificate_path: impl AsRef<Path>,
        max_kes_evolutions: u64,
    ) -> Result<Self, ProcessCredentialsError> {
        let vrf = vrf::SecretKey::from_file(vrf_path).map_err(CredentialsError::Vrf)?;
        let (operational_cert, cold) =
            load_operational_certificate(certificate_path).map_err(CredentialsError::Certificate)?;
        let issuer = cold_verifying_key(&cold).ok_or(CredentialsError::ColdKey)?;
        AssertOperationalCertificateError::new(
            &operational_cert,
            &issuer,
            Some(operational_cert.operational_cert_sequence_number),
        )
        .map_err(|error| CredentialsError::Signature(error.to_string()))?;

        let start_period = operational_cert.operational_cert_kes_period;
        let config = SignerConfig {
            executable: std::env::current_exe()?
                .with_file_name(format!("amaru-kes-signer{}", std::env::consts::EXE_SUFFIX)),
            key_path: kes_path.as_ref().to_path_buf(),
            start_period,
            max_kes_evolutions,
            expected_key: operational_cert.operational_cert_hot_verification_key,
        };
        let process = SignerProcess::spawn(&config).map_err(ProcessCredentialsError::Signer)?;
        let (signer, requests) = mpsc::channel();
        thread::Builder::new()
            .name("amaru-kes-signer".into())
            .spawn(move || supervise_signer(requests, process, config, SIGN_RESPONSE_TIMEOUT))?;

        Ok(Self {
            issuer: IssuerFields {
                issuer_verification_key: cold,
                vrf_verification_key: VerificationKey::from(*vrf::PublicKey::from(&vrf)),
                operational_cert,
            },
            vrf,
            max_kes_evolutions,
            signer,
            last_logged_period: Mutex::new(start_period),
        })
    }
}

impl ForgingCredentials for ProcessCredentials {
    fn issuer_verification_key(&self) -> VerificationKey {
        self.issuer.issuer_verification_key
    }

    fn vrf_secret_bytes(&self) -> [u8; 32] {
        self.vrf.to_bytes()
    }

    fn issuer_fields(&self) -> IssuerFields {
        self.issuer.clone()
    }

    fn sign(&self, period: KesPeriod, message: &[u8]) -> Result<KesSignature, ForgingCredentialsError> {
        period.evolutions_since(self.issuer.operational_cert.operational_cert_kes_period, self.max_kes_evolutions)?;
        let (reply, response) = mpsc::channel();
        self.signer
            .send(SignRequest { period, message: message.to_vec(), reply })
            .map_err(|_| ForgingCredentialsError::Kes("KES signer supervisor stopped".into()))?;
        let signature = response
            .recv_timeout(SIGN_RESPONSE_TIMEOUT)
            .map_err(|error| ForgingCredentialsError::Kes(format!("KES signer unavailable: {error}")))?
            .map_err(ForgingCredentialsError::Kes)?;
        let mut logged = self.last_logged_period.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if *logged != period {
            *logged = period;
            amaru_observability::info!(consensus::forge::KES_PERIOD, period);
        }
        Ok(signature)
    }
}

/// Load complete forging inputs on a testnet, or remain a follower without them.
pub fn process_forging_credentials_from_files(
    network: NetworkName,
    max_kes_evolutions: u64,
    kes_path: Option<&Path>,
    vrf_path: Option<&Path>,
    certificate_path: Option<&Path>,
) -> Result<Option<ProcessCredentials>, ProcessCredentialsError> {
    match (network, kes_path, vrf_path, certificate_path) {
        (_, None, None, None) => Ok(None),
        (NetworkName::Mainnet, _, _, _) => Err(CredentialsError::Mainnet.into()),
        (_, Some(kes), Some(vrf), Some(certificate)) => {
            Ok(Some(ProcessCredentials::from_files(kes, vrf, certificate, max_kes_evolutions)?))
        }
        _ => Err(CredentialsError::PartialFlags.into()),
    }
}

#[derive(Debug, Error)]
pub enum ProcessCredentialsError {
    #[error(transparent)]
    Credentials(#[from] CredentialsError),
    #[error("could not start KES signer: {0}")]
    Io(#[from] io::Error),
    #[error("KES signer: {0}")]
    Signer(String),
}

struct SignRequest {
    period: KesPeriod,
    message: Vec<u8>,
    reply: Sender<Result<KesSignature, String>>,
}

#[derive(Serialize, Deserialize)]
struct WireRequest {
    period: KesPeriod,
    message: Vec<u8>,
}

enum SignerRequestError {
    Rejected(String),
    Transport(String),
}

impl SignerRequestError {
    fn into_message(self) -> String {
        match self {
            Self::Rejected(message) | Self::Transport(message) => message,
        }
    }
}

struct SignerProcess {
    child: Child,
    io: Option<SignerIo>,
}

struct SignerIo {
    input: BufWriter<ChildStdin>,
    output: BufReader<ChildStdout>,
}

struct SignerConfig {
    executable: PathBuf,
    key_path: PathBuf,
    start_period: KesPeriod,
    max_kes_evolutions: u64,
    expected_key: VerificationKey,
}

impl SignerProcess {
    fn spawn(config: &SignerConfig) -> Result<Self, String> {
        let mut child = Command::new(&config.executable)
            .arg(&config.key_path)
            .arg(u64::from(config.start_period).to_string())
            .arg(config.max_kes_evolutions.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|error| error.to_string())?;
        let (input, output) = read_signer_handshake(&mut child, config.expected_key, SIGNER_STARTUP_TIMEOUT)?;
        Ok(Self { child, io: Some(SignerIo { input, output }) })
    }

    fn sign(
        &mut self,
        period: KesPeriod,
        message: &[u8],
        timeout: Duration,
    ) -> Result<KesSignature, SignerRequestError> {
        let mut io = self.io.take().ok_or_else(|| SignerRequestError::Transport("KES signer unavailable".into()))?;
        let message = message.to_vec();
        let (reply, response) = mpsc::sync_channel(1);
        thread::Builder::new()
            .name("amaru-kes-signer-request".into())
            .spawn(move || {
                let result = io.sign(period, &message);
                let _ = reply.send((io, result));
            })
            .map_err(|error| SignerRequestError::Transport(error.to_string()))?;
        let (io, result) = response.recv_timeout(timeout).map_err(|error| match error {
            RecvTimeoutError::Timeout => {
                SignerRequestError::Transport(format!("KES signer request timed out after {timeout:?}"))
            }
            RecvTimeoutError::Disconnected => SignerRequestError::Transport("KES signer request worker stopped".into()),
        })?;
        self.io = Some(io);
        result
    }
}

impl SignerIo {
    fn sign(&mut self, period: KesPeriod, message: &[u8]) -> Result<KesSignature, SignerRequestError> {
        let request = WireRequest { period, message: message.to_vec() };
        serde_json::to_writer(&mut self.input, &request)
            .map_err(|error| SignerRequestError::Transport(error.to_string()))?;
        self.input.write_all(b"\n").map_err(|error| SignerRequestError::Transport(error.to_string()))?;
        self.input.flush().map_err(|error| SignerRequestError::Transport(error.to_string()))?;
        let mut line = String::new();
        if self.output.read_line(&mut line).map_err(|error| SignerRequestError::Transport(error.to_string()))? == 0 {
            return Err(SignerRequestError::Transport("KES signer closed its output".into()));
        }
        let response: Result<KesSignature, String> =
            serde_json::from_str(&line).map_err(|error| SignerRequestError::Transport(error.to_string()))?;
        response.map_err(SignerRequestError::Rejected)
    }
}

fn read_signer_handshake(
    child: &mut Child,
    expected_key: VerificationKey,
    timeout: Duration,
) -> Result<(BufWriter<ChildStdin>, BufReader<ChildStdout>), String> {
    let deadline = Instant::now() + timeout;
    let handshake = (|| {
        let input = BufWriter::new(child.stdin.take().ok_or("KES signer stdin unavailable")?);
        let output = child.stdout.take().ok_or("KES signer stdout unavailable")?;
        let (reply, response) = mpsc::sync_channel(1);
        thread::Builder::new()
            .name("amaru-kes-signer-handshake".into())
            .spawn(move || {
                let mut output = BufReader::new(output);
                let mut line = String::new();
                let result = output.read_line(&mut line);
                let _ = reply.send((output, line, result));
            })
            .map_err(|error| error.to_string())?;
        let (output, line, result) =
            response.recv_timeout(deadline.saturating_duration_since(Instant::now())).map_err(|error| match error {
                RecvTimeoutError::Timeout => format!("KES signer handshake timed out after {timeout:?}"),
                RecvTimeoutError::Disconnected => "KES signer handshake reader stopped".into(),
            })?;
        if result.map_err(|error| error.to_string())? == 0 {
            return Err("KES signer closed its output before the handshake".into());
        }
        let public_key: VerificationKey =
            serde_json::from_str(&line).map_err(|error| format!("KES signer handshake: {error}"))?;
        if public_key != expected_key {
            return Err("KES verification key does not match the operational certificate hot key".into());
        }
        Ok((input, output))
    })();
    if handshake.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    handshake
}

impl Drop for SignerProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn supervise_signer(
    requests: Receiver<SignRequest>,
    first: SignerProcess,
    config: SignerConfig,
    request_timeout: Duration,
) {
    let mut process = Some(first);
    let mut last_attempt = Instant::now();
    loop {
        if let Some(signer) = process.as_mut() {
            match signer.child.try_wait() {
                Ok(Some(_)) | Err(_) => {
                    process = None;
                    amaru_observability::warn!(
                        consensus::forge::SIGNER_STATUS,
                        status = "exited",
                        detail = "restarting"
                    );
                }
                Ok(None) => {}
            }
        }
        if process.is_none() && last_attempt.elapsed() >= RESTART_INTERVAL {
            last_attempt = Instant::now();
            match SignerProcess::spawn(&config) {
                Ok(signer) => {
                    process = Some(signer);
                    amaru_observability::info!(consensus::forge::SIGNER_STATUS, status = "ready", detail = "restarted");
                }
                Err(error) => {
                    amaru_observability::warn!(consensus::forge::SIGNER_STATUS, status = "unavailable", detail = error);
                }
            }
        }
        match requests.recv_timeout(SUPERVISOR_POLL_INTERVAL) {
            Ok(request) => {
                let result = match process.as_mut() {
                    Some(signer) => signer.sign(request.period, &request.message, request_timeout),
                    None => Err(SignerRequestError::Transport("KES signer unavailable".into())),
                };
                if matches!(result, Err(SignerRequestError::Transport(_))) {
                    process = None;
                }
                let _ = request.reply.send(result.map_err(SignerRequestError::into_message));
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

/// Run the private-key holder in the child process. Standard output is reserved for IPC.
pub fn run_kes_signer(
    key_path: &Path,
    start_period: KesPeriod,
    max_kes_evolutions: u64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut key = kes::SecretKey::from_file(key_path)?;
    let public_key = VerificationKey::from(*kes::PublicKey::from(&mut key));
    let input = io::stdin();
    let mut input = input.lock();
    let output = io::stdout();
    let mut output = output.lock();
    serde_json::to_writer(&mut output, &public_key)?;
    output.write_all(b"\n")?;
    output.flush()?;

    let mut line = String::new();
    while input.read_line(&mut line)? != 0 {
        let response = (|| -> Result<KesSignature, String> {
            let request: WireRequest = serde_json::from_str(&line).map_err(|error| error.to_string())?;
            let evolution =
                request.period.evolutions_since(start_period, max_kes_evolutions).map_err(|error| error.to_string())?;
            key.evolve_to(evolution).map_err(|error| error.to_string())?;
            Ok(KesSignature::from(<[u8; kes::Signature::SIZE]>::from(&key.sign(&request.message))))
        })();
        serde_json::to_writer(&mut output, &response)?;
        output.write_all(b"\n")?;
        output.flush()?;
        line.clear();
    }
    Ok(())
}
