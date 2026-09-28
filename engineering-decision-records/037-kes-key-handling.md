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

When the KES key validity period is about to expire, Amaru will generate WARN messages to alert the SPO that they needs to rotate their KES key.
The Amaru TUI will show a count-down and the end of the validity period in local and UTC time.

### Key storage

Amaru is not opinionated how the cold key is stored.
The SPO uses any mechanism they deems adequate to compute the cold key signature on the KES certificate.

The KES key and its certificate are stored in the chain database (because block forging is the quintessential consensus activity that provides chain safety, liveness, and security guarantees).
The key may be encrypted with a password, which would then need to be provided by the SPO when starting Amaru; environment variables are not suitable for this purpose, reading from a file is (which might be a FIFO or a unix socket or a terminal).

### Key usage

When a KES key is available, the Amaru process will fork and thus run a separate child process responsible solely for holding that key and using it to sign blocks.
Communication with the main Amaru process is done in the idiomatic form of IPC for the platform.
For this purpose, an implementation of `trait CredentialsResource` is provided.

### Key rotation

The SPO uses `amaru keys hot create` to generate a new KES key and print the corresponding certificate request to stdout; it optionally asks the SPO for a password to encrypt the key.
The SPO then uses their cold key to sign the certificate request.
The SPO then uses `amaru keys hot import` to import the KES certificate signature into the chain database.

Note that the chain database can hold multiple KES keys and certificates, each valid for a certain range of slots.

### Cold key handling

While many SPOs nowadays use hardware wallets to store their cold keys, Amaru comes with a simple tool that can be used e.g. on an air-gapped machine; while this doesn’t achieve the same level of security, we want to provide a complete set of tooling to get started.
`amaru keys cold create` generates a new cold key, writes it to a file (which requires a password for encryption), and prints the corresponding public key etc. to stdout.
`amaru keys cold sign` reads a certificate request from a file, prompts the SPO for the password to decrypt the cold key, and writes the signature to stdout.

## Consequences

- KES key rotation can be performed while the node is running, Amaru will check for the presence of new KES keys as required.
- The SPO can organise the rotation workflow as they pleases, as long as Amaru uses well-known import / export formats for signing requests and signatures.
- Hot keys are reasonably well protected because they are not available in the same memory address space that also performs network operations and processes potentially malicious inputs.
  We may later look into using TPM or similar hardware if desired.

[edr-forging]: ./036-block-forging.md
