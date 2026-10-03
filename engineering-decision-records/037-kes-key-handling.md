---
type: architecture
status: accepted
---

# KES Key Handling

With the ability of [block forging][edr-forging] arises the need for safe KES key handling.
Each stake-pool operator (SPO) is responsible for their KES keys and is expected by the network to follow best practices.
The practices adopted by Cardano SPOs vary a bit, which means that our approach needs to be flexible enough to accommodate different workflows.

## Context

A stake pool is identified by a hash of the public key corresponding to the cold key.
This cold key needs to be protected stringently since it cannot be changed without changing the identity of the stake pool and thereby losing all delegated stake.
The block signing protocol thus uses a separate KES key, also called the “hot key”, which must be replaced periodically (roughly every three months on current mainnet).
This KES key is linked to the pool identity via a certificate signed by the cold key, which is included in the block header.

## Decision

### SPO support

For every KES period change, an INFO event is logged; this can be used by the SPO to monitor the progression, trigger alerts, etc.
When the KES key validity period is about to expire, Amaru will generate WARN messages to alert the SPO that they needs to rotate their KES key.
The Amaru TUI will show a count-down and the end of the validity period in local and UTC time.

### Key storage

Amaru is not opinionated how the cold key is stored.
The SPO uses any mechanism they deems adequate to compute the cold key signature on the KES certificate.

For initial block forging support, the SPO provides filesystem paths for the KES signing key, VRF signing key, and operational certificate to `amaru node run`.
The certificate file also contains the cold verification key; Amaru never needs the cold signing key.
The KES key is not stored in the chain database.
Initial support accepts unencrypted cardano-cli text envelopes on testnets.
Password-encrypted KES keys may be added later; passwords must be read from a file-like source rather than an environment variable.

### Key usage

When a KES key is configured, Amaru launches the `amaru-kes-signer` executable installed beside it. That process opens and holds the KES key and signs header-body bytes on request.
The node process retains the VRF key for leader scheduling and the public certificate fields for header construction and validation.
The child communicates with the node through piped IPC and returns its KES verification key at startup so the node can check it against the certificate.
If the child exits, Amaru restarts it and checks the KES verification key again; a slot whose signing request fails is missed without shutting down the node.

### Key rotation

For initial support, the SPO generates a cardano-cli compatible KES key pair with `amaru keys kes create` or `cardano-cli node key-gen-KES`.
The SPO issues an operational certificate with their existing offline workflow, deploys the files on the block producer, and restarts Amaru with their paths.

Amaru checks the new certificate's sequence number against the adopted chain on startup.

### Cold key handling

Cold-key creation and operational certificate signing stay in the SPO's existing offline workflow.
Amaru does not read or generate the cold signing key for initial support.

## Consequences

- KES key rotation takes effect when the SPO restarts Amaru with the new files.
- The SPO can organise the rotation workflow as they pleases, as long as Amaru uses well-known import / export formats for signing requests and signatures.
- Hot keys are reasonably well protected because they are not available in the same memory address space that also performs network operations and processes potentially malicious inputs.
  We may later look into using TPM or similar hardware if desired.
- It will be straight-forward to add an HTTP endpoint on localhost to allow other tooling to interact with the hot key process.

## Discussion points

- Should Amaru offer import or export facilities for cold or hot keys to interact with other tooling? If yes, which tools and formats exactly?

[edr-forging]: ./035-block-forging.md
