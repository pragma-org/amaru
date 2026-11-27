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

//! A listener keeps accepting after its accept failed for lack of file descriptors (EMFILE).
//!
//! The test runs itself again in a child process with a low descriptor limit, so filling the
//! descriptor table does not affect other tests.
#![cfg(unix)]

use std::{net::SocketAddr, process::Command, time::Duration};

use amaru_network::connection::TokioConnections;
use amaru_ouroboros::ConnectionProvider;

const CHILD: &str = "AMARU_NETWORK_ACCEPT_OUT_OF_DESCRIPTORS_CHILD";
const TEST: &str = "listener_accepts_again_after_running_out_of_descriptors";

#[test]
fn listener_accepts_again_after_running_out_of_descriptors() -> anyhow::Result<()> {
    if std::env::var_os(CHILD).is_some() {
        return tokio::runtime::Builder::new_multi_thread().enable_all().build()?.block_on(child());
    }
    let status = Command::new("sh")
        .arg("-c")
        .arg(r#"ulimit -n 256 && exec "$0" --exact "$1" --nocapture"#)
        .arg(std::env::current_exe()?)
        .arg(TEST)
        .env(CHILD, "1")
        .status()?;
    assert!(status.success(), "child test failed: {status}");
    Ok(())
}

async fn child() -> anyhow::Result<()> {
    let connections = TokioConnections::new(1024);
    let addr = connections.listen(SocketAddr::from(([127, 0, 0, 1], 0))).await?;

    // Fill the descriptor table, leaving one descriptor for the client socket, so the listener
    // has none left to accept the connection with.
    let mut hogs = Vec::new();
    while let Ok(file) = std::fs::File::open("/dev/null") {
        hogs.push(file);
    }
    hogs.pop();
    let _first = std::net::TcpStream::connect(addr)?;
    tokio::time::sleep(Duration::from_millis(200)).await;

    drop(hogs);
    let second = std::net::TcpStream::connect(addr)?;
    let mut accepted = Vec::new();
    for _ in 0..2 {
        let (peer, _) = tokio::time::timeout(Duration::from_secs(5), connections.accept(addr)).await??;
        accepted.push(SocketAddr::from(peer));
    }
    assert!(accepted.contains(&second.local_addr()?), "{accepted:?}");
    Ok(())
}
