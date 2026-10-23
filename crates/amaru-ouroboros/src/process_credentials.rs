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

#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::{
    io::{self, BufRead, BufReader, BufWriter, Write},
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
    },
    thread,
    time::Duration,
};

use amaru_kernel::{KesPeriod, KesSignature, NetworkName, VerificationKey};
use amaru_ouroboros_traits::{ForgingCredentials, ForgingCredentialsError, IssuerFields};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::{
    credentials::{CredentialsError, load_operational_certificate, validate_operational_certificate},
    kes, vrf,
};

const RESTART_INTERVAL: Duration = Duration::from_secs(1);
const SIGNER_STARTUP_TIMEOUT: Duration = Duration::from_secs(5);
const SIGN_RESPONSE_TIMEOUT: Duration = Duration::from_secs(2);

/// The node's public and VRF credentials, with KES signing delegated to a child.
///
/// The KES path is passed to the child unopened. No KES secret is decoded in this process.
/// Both processes run as the same user, so the parent retains access to the signing-key file.
/// A compromised parent can still read the key from that path.
pub struct ProcessCredentials {
    vrf: vrf::SecretKey,
    issuer: IssuerFields,
    signer: Arc<Mutex<Option<SignerProcess>>>,
    stop: Sender<()>,
    supervisor: Option<thread::JoinHandle<()>>,
    last_logged_period: Mutex<KesPeriod>,
}

impl ProcessCredentials {
    /// Load the VRF key and certificate, then start and verify the KES signer beside this executable.
    pub fn from_files(
        kes_path: impl AsRef<Path>,
        vrf_path: impl AsRef<Path>,
        certificate_path: impl AsRef<Path>,
        max_kes_evolutions: u64,
    ) -> Result<Self, CredentialsError> {
        let vrf = vrf::SecretKey::from_file(vrf_path)?;
        let (operational_cert, cold) = load_operational_certificate(certificate_path)?;
        validate_operational_certificate(&operational_cert, &cold)?;

        let start_period = operational_cert.operational_cert_kes_period;
        let expected_key = operational_cert.operational_cert_hot_verification_key;
        let mut command = Command::new(
            std::env::current_exe()?.with_file_name(format!("amaru-kes-signer{}", std::env::consts::EXE_SUFFIX)),
        );
        command
            .arg(kes_path.as_ref())
            .arg(u64::from(start_period).to_string())
            .arg(max_kes_evolutions.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
        #[cfg(unix)]
        command.process_group(0);
        let signer = Arc::new(Mutex::new(Some(
            SignerProcess::spawn(&mut command, expected_key).map_err(CredentialsError::Signer)?,
        )));
        let (stop, shutdown) = mpsc::channel();
        let supervised = Arc::clone(&signer);
        let supervisor = thread::Builder::new()
            .name("amaru-kes-signer".into())
            .spawn(move || supervise_signer(supervised, command, expected_key, shutdown))?;

        Ok(Self {
            issuer: IssuerFields {
                issuer_verification_key: cold,
                vrf_verification_key: VerificationKey::from(*vrf::PublicKey::from(&vrf)),
                operational_cert,
            },
            vrf,
            signer,
            stop,
            supervisor: Some(supervisor),
            last_logged_period: Mutex::new(start_period),
        })
    }

    /// Return a non-blocking callback that stops restarts and requests signer termination.
    /// Dropping the credentials waits for the supervisor to kill and reap the child.
    pub fn shutdown_callback(&self) -> impl Fn() + Send + Sync + 'static {
        let stop = self.stop.clone();
        move || {
            let _ = stop.send(());
        }
    }
}

impl Drop for ProcessCredentials {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(supervisor) = self.supervisor.take() {
            let _ = supervisor.join();
        }
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
        let mut signer = self.signer.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let result = signer
            .as_ref()
            .ok_or_else(|| ForgingCredentialsError::Kes("KES signer unavailable".into()))
            .and_then(|signer| signer.sign(WireRequest { period, message: message.to_vec() }));
        if matches!(result, Err(ForgingCredentialsError::Kes(_) | ForgingCredentialsError::EvolvedPast(_))) {
            *signer = None;
        }
        let signature = result?;
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
) -> Result<Option<ProcessCredentials>, CredentialsError> {
    match (network, kes_path, vrf_path, certificate_path) {
        (_, None, None, None) => Ok(None),
        (NetworkName::Mainnet, _, _, _) => Err(CredentialsError::Mainnet),
        (_, Some(kes), Some(vrf), Some(certificate)) => {
            Ok(Some(ProcessCredentials::from_files(kes, vrf, certificate, max_kes_evolutions)?))
        }
        _ => Err(CredentialsError::PartialFlags),
    }
}

#[derive(Serialize, Deserialize)]
struct WireRequest {
    period: KesPeriod,
    message: Vec<u8>,
}

struct SignerProcess {
    child: Child,
    requests: Sender<String>,
    responses: Receiver<io::Result<String>>,
}

impl SignerProcess {
    fn spawn(command: &mut Command, expected_key: VerificationKey) -> Result<Self, String> {
        let (requests, incoming) = mpsc::channel::<String>();
        let (reply, responses) = mpsc::channel();
        let mut signer = Self { child: command.spawn().map_err(|error| error.to_string())?, requests, responses };
        let mut input = BufWriter::new(signer.child.stdin.take().ok_or("KES signer stdin unavailable")?);
        let output = BufReader::new(signer.child.stdout.take().ok_or("KES signer stdout unavailable")?);
        thread::Builder::new()
            .name("amaru-kes-signer-io".into())
            .spawn(move || {
                for line in output.lines() {
                    if reply.send(line).is_err() {
                        break;
                    }
                    let Ok(request) = incoming.recv() else { break };
                    if let Err(error) = writeln!(input, "{request}").and_then(|()| input.flush()) {
                        let _ = reply.send(Err(error));
                        break;
                    }
                }
            })
            .map_err(|error| error.to_string())?;
        if signer.receive::<VerificationKey>(SIGNER_STARTUP_TIMEOUT)? != expected_key {
            return Err("KES verification key does not match the operational certificate hot key".into());
        }
        Ok(signer)
    }

    fn receive<T: DeserializeOwned>(&self, timeout: Duration) -> Result<T, String> {
        let line = self
            .responses
            .recv_timeout(timeout)
            .map_err(|error| format!("KES signer response: {error}"))?
            .map_err(|error| error.to_string())?;
        serde_json::from_str(&line).map_err(|error| error.to_string())
    }

    fn sign(&self, request: WireRequest) -> Result<KesSignature, ForgingCredentialsError> {
        let request =
            serde_json::to_string(&request).map_err(|error| ForgingCredentialsError::Kes(error.to_string()))?;
        self.requests
            .send(request)
            .map_err(|_| ForgingCredentialsError::Kes("KES signer I/O worker stopped".into()))?;
        self.receive(SIGN_RESPONSE_TIMEOUT).map_err(ForgingCredentialsError::Kes)?
    }
}

impl Drop for SignerProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn supervise_signer(
    signer: Arc<Mutex<Option<SignerProcess>>>,
    mut command: Command,
    expected_key: VerificationKey,
    stop: Receiver<()>,
) {
    while matches!(stop.recv_timeout(RESTART_INTERVAL), Err(RecvTimeoutError::Timeout)) {
        let mut process = signer.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if stop.try_recv().is_ok() {
            break;
        }
        if process.as_mut().is_some_and(|signer| !matches!(signer.child.try_wait(), Ok(None))) {
            *process = None;
            amaru_observability::warn!(consensus::forge::SIGNER_STATUS, status = "exited", detail = "restarting");
        }
        if process.is_none() {
            drop(process);
            match SignerProcess::spawn(&mut command, expected_key) {
                Ok(restarted) => {
                    *signer.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(restarted);
                    amaru_observability::info!(consensus::forge::SIGNER_STATUS, status = "ready", detail = "restarted");
                }
                Err(error) => {
                    amaru_observability::warn!(consensus::forge::SIGNER_STATUS, status = "unavailable", detail = error);
                }
            }
        }
    }
    signer.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take();
}

/// Run the private-key holder in the child process. Standard output is reserved for IPC.
pub fn run_kes_signer(
    key_path: &Path,
    start_period: KesPeriod,
    max_kes_evolutions: u64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut key = kes::SecretKey::from_file(key_path)?;
    let public_key = VerificationKey::from(*kes::PublicKey::from(&mut key));
    let mut output = io::stdout().lock();
    serde_json::to_writer(&mut output, &public_key)?;
    output.write_all(b"\n")?;
    output.flush()?;

    for line in io::stdin().lock().lines() {
        let line = line?;
        let response = (|| -> Result<KesSignature, ForgingCredentialsError> {
            let request: WireRequest =
                serde_json::from_str(&line).map_err(|error| ForgingCredentialsError::Kes(error.to_string()))?;
            let evolution = request.period.evolutions_since(start_period, max_kes_evolutions)?;
            key.evolve_to(evolution).map_err(|error| {
                if matches!(error, kes::KesError::CannotEvolveBackwards { .. }) {
                    ForgingCredentialsError::EvolvedPast(error.to_string())
                } else {
                    ForgingCredentialsError::Kes(error.to_string())
                }
            })?;
            Ok(KesSignature::from(<[u8; kes::Signature::SIZE]>::from(&key.sign(&request.message))))
        })();
        serde_json::to_writer(&mut output, &response)?;
        output.write_all(b"\n")?;
        output.flush()?;
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use std::time::Instant;

    use super::*;

    #[test]
    fn signer_rejects_mismatched_keys_and_times_out() {
        let key = VerificationKey::zeroes();
        let mut command = Command::new("sh");
        command
            .args(["-c", r#"printf '%s\n' "$KEY"; read -r request; read -r request"#])
            .env("KEY", serde_json::to_string(&key).unwrap())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
        assert!(SignerProcess::spawn(&mut command, VerificationKey::from([1; 32])).is_err());
        let signer = SignerProcess::spawn(&mut command, key).unwrap();
        let error = signer.sign(WireRequest { period: 0.into(), message: vec![] }).unwrap_err();
        assert!(error.to_string().contains("timed out"));
        assert!(
            SignerProcess::spawn(
                Command::new("sh").args(["-c", "read -r request"]).stdin(Stdio::piped()).stdout(Stdio::piped()),
                key
            )
            .err()
            .unwrap()
            .contains("timed out")
        );
    }

    #[test]
    fn supervisor_restarts_an_exited_signer_and_stops_on_shutdown() {
        let key = VerificationKey::zeroes();
        let mut command = Command::new("sh");
        command
            .args(["-c", r#"printf '%s\n' "$KEY"; read -r request"#])
            .env("KEY", serde_json::to_string(&key).unwrap())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
        let mut first = SignerProcess::spawn(&mut command, key).unwrap();
        let pid = first.child.id();
        first.child.kill().unwrap();
        let signer = Arc::new(Mutex::new(Some(first)));
        let supervised = Arc::clone(&signer);
        let (stop, shutdown) = mpsc::channel();
        let supervisor = thread::spawn(move || supervise_signer(supervised, command, key, shutdown));
        let deadline = Instant::now() + SIGNER_STARTUP_TIMEOUT;
        while signer.lock().unwrap().as_ref().is_none_or(|signer| signer.child.id() == pid) {
            assert!(Instant::now() < deadline, "signer was not restarted");
            thread::sleep(Duration::from_millis(10));
        }
        stop.send(()).unwrap();
        supervisor.join().unwrap();
        assert!(signer.lock().unwrap().is_none());
    }
}
