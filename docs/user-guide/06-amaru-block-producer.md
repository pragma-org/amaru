---
id: amaru-block-producer
title: "Running an Amaru Node on Mainnet: Block Producer"
sidebar_label: "Block Producer"
description: Turn an Amaru node into a stake pool block producer, from keys and operational certificate to configuration, verification and KES key rotation.
---

An Amaru node forges blocks when it is given three files: a KES signing key, a VRF signing key and an operational certificate. This page covers creating those files with the `amaru pool` commands, configuring the node, checking that it forges, and rotating the KES key.

It assumes a node installed as described in [Advanced Installation](05-amaru-advanced-installation.md), on a host hardened as in [System Hardening](04-amaru-advanced-hardening.md), and at least one relay of your own.

:::note Pool registration
Registering the pool, and later updating its registration, still goes through `cardano-cli`. See [Register your stake pool](https://developers.cardano.org/docs/operators/block-producer/register-stake-pool/). Existing pools can keep their `cardano-cli` files: Amaru reads them as they are.
:::

:::warning Keep the cold key offline
The cold signing key and its issue counter never belong on the block producer. Every command marked **offline** below runs on an air-gapped machine with the `amaru` binary on it. Only the KES signing key, the VRF signing key and the operational certificate go to the block producer.
:::

## 1. The files a block producer needs

| File | Where it lives | `amaru node run` flag | Environment variable | Accepted formats |
|:-----|:---------------|:-----------------|:---------------------|:-----------------|
| KES signing key | block producer | `--operator-kes` | `AMARU_OPERATOR_KES` | raw bytes, or a `cardano-cli` `kes.skey` text envelope |
| VRF signing key | block producer | `--operator-vrf` | `AMARU_OPERATOR_VRF` | raw 32-byte seed, or an unencrypted `cardano-cli` `vrf.skey` text envelope |
| Operational certificate | block producer | `--operator-cert` | `AMARU_OPERATOR_CERT` | binary CBOR, or a `cardano-cli` `node.cert` text envelope |
| Cold signing key | offline | – | – | raw 32-byte seed, or an unencrypted `cardano-cli` text envelope |
| Cold issue counter | offline | – | – | binary CBOR, or a `cardano-cli` text envelope |

Pass all three of `--operator-kes`, `--operator-vrf` and `--operator-cert`, or none: with none the node runs as a relay, and with only some of them it refuses to start. The operational certificate includes the cold verification key, so the node does not read a separate cold key file.

## 2. Creating the keys of a new pool

Skip this section for an existing pool: its `cardano-cli` keys work with Amaru.

**Offline**, create the cold key and its issue counter:

```bash
amaru pool create --network mainnet \
  --cold-signing-key cold.skey \
  --cold-verification-key cold.vkey \
  --counter cold.counter
```

```json
{
  "network": "mainnet",
  "cold_verification_key": "86920acb…",
  "pool_id": "pool1…"
}
```

`amaru pool create` can also generate payment and owner/reward stake keys (`--payment-signing-key`, `--payment-verification-key`, `--stake-signing-key`, `--stake-verification-key`). `amaru pool cold create` creates only the cold key and counter. Derive the pool ID again at any time with `amaru pool id inspect --verification-key cold.vkey`.

On the **block producer**, create the VRF and KES key pairs:

```bash
amaru pool vrf create --signing-key vrf.skey --verification-key vrf.vkey
amaru pool kes create --signing-key kes.skey --verification-key kes.vkey
```

The keys are written with mode `0600`, and an existing file is never overwritten (`kes.skey already exists`). `amaru pool vrf inspect --verification-key vrf.vkey` prints the VRF key hash.

All keys are raw bytes. `cardano-cli` reads text envelopes, so for the registration certificate wrap the two verification keys it needs:

```bash
jq -n --arg h "5820$(xxd -p -c 64 cold.vkey)" \
  '{type: "StakePoolVerificationKey_ed25519", description: "Stake Pool Operator Verification Key", cborHex: $h}' > cold.vkey.json
jq -n --arg h "5820$(xxd -p -c 64 vrf.vkey)" \
  '{type: "VrfVerificationKey_PraosVRF", description: "VRF Verification Key", cborHex: $h}' > vrf.vkey.json
```

`cardano-cli latest stake-pool id --cold-verification-key-file cold.vkey.json` and `cardano-cli latest node key-hash-VRF --verification-key-file vrf.vkey.json` then print the same pool ID and VRF key hash as Amaru.

## 3. Issuing the operational certificate

The certificate starts at the current KES period. Read it from the running node's [metrics](05-amaru-advanced-installation.md#5-logs-metrics-and-monitoring):

```bash
curl -s 127.0.0.1:8889/metrics | grep -E '^cardano_node_metrics_currentKESPeriod_int'
```

Read it from a node that is in sync: the metric follows the last applied block. Alternatively, divide the current slot by the number of slots per KES period (129600 on mainnet, preprod and preview).

Copy `kes.vkey` to the offline machine. Then, **offline**:

```bash
amaru pool cert create \
  --cert op.cert \
  --cold-signing-key cold.skey \
  --counter cold.counter \
  --kes-period <CURRENT_KES_PERIOD> \
  --kes-verification-key kes.vkey

amaru pool cert inspect --cert op.cert
```

```json
{
  "pool_id": "pool1…",
  "sequence_number": 0,
  "kes_period": 991,
  "kes_verification_key": "9f95de8c…"
}
```

`amaru pool cert create` advances the issue counter by one with each certificate. It also keeps a `cold.counter.lock` file next to the counter. Leave that file in place, and don't issue certificates for the same counter with another tool at the same time. Before it advances the counter, it saves the new certificate as `cold.counter.cert-recovery`, and removes that file once `op.cert` is written. If an interrupted run leaves it behind, later runs on that counter refuse to start: recover the certificate from it, check the counter, then remove the file. Existing certificates are never replaced: pick a new file name, or move the old one first.

`amaru pool kes period` shows how long a certificate stays valid:

```bash
amaru pool kes period --cert op.cert --slot <CURRENT_SLOT>
```

```json
{
  "kes_period_current": 991,
  "kes_period_cert": 991,
  "periods_used": 0,
  "periods_remaining": 62
}
```

:::warning Counters written by Amaru are binary
`amaru pool cert create` reads a `cardano-cli` text-envelope counter, but always writes the counter back as binary CBOR. After that, `cardano-cli node issue-op-cert` cannot read it any more (`TextEnvelope aeson decode error`). If you might go back to `cardano-cli`, keep a copy of the text-envelope counter before the first Amaru issuance. A new certificate's sequence number must be equal to, or one higher than, the last one the chain has seen from your pool. The node checks this at startup and refuses to start with a certificate that doesn't fit.
:::

Copy `op.cert` to the block producer.

## 4. Configuring the node as a block producer

Put the three files in a folder that only the `amaru` user can read:

```bash
sudo install -d -m 0700 -o amaru -g amaru /var/lib/amaru/pool
sudo install -m 0400 -o amaru -g amaru kes.skey vrf.skey op.cert /var/lib/amaru/pool/
```

Point the node at them, and let it talk to your own relays only:

```bash title="/etc/default/amaru"
AMARU_OPERATOR_KES=/var/lib/amaru/pool/kes.skey
AMARU_OPERATOR_VRF=/var/lib/amaru/pool/vrf.skey
AMARU_OPERATOR_CERT=/var/lib/amaru/pool/op.cert

AMARU_PEER=relay1.example.com:3001,relay2.example.com:3001
AMARU_PEERS_MIX="static!2"
AMARU_PEERS_MAX_UPSTREAM=2
AMARU_PEERS_MAX_DOWNSTREAM=4
```

With `static!2` as the only term of the mix, the node connects only to the peers in `AMARU_PEER` (here two relays) and does not look for ledger or shared peers. Keep `AMARU_PEERS_MAX_UPSTREAM` at the number of relays.

On the relays, add the block producer to their `AMARU_PEER` list. In the block producer's firewall, accept the node port only from the relays. With the ruleset from [System Hardening](04-amaru-advanced-hardening.md#firewall--nftables), replace `tcp dport 3001 accept` with:

```bash title="/etc/nftables.conf"
ip saddr { RELAY1_IP, RELAY2_IP } tcp dport 3001 accept
```

Then restart the node:

```bash
sudo systemctl restart amaru
```

:::danger One node per set of keys
Never run two nodes with the same KES key and operational certificate at the same time. Stop the old block producer before you start the new one.
:::

## 5. Checking that the node forges

### Logs

At startup, a node with forging keys logs `build.forging` with the `pool_id`, the certificate's `sequence` and its `kes_period`. The forging events after that all start with `forge.`:

```bash
journalctl -u amaru -f | grep -F 'forge.'
```

| Event | Level | Meaning |
|:------|:------|:--------|
| `forge.kes_period` | INFO | The KES key was evolved to a new KES period (`period`) while signing a block |
| `forge.schedule` | INFO | Led slots still ahead in each epoch the node holds a schedule for (`slots`), and the time of the next one (`next_slot`). DEBUG while the node is syncing, and not logged when no led slot is left |
| `forge.block_body` | INFO | Transactions selected for the block (`slot`, `tx_count`, `size_bytes`) |
| `forge.forged` | INFO | A block was forged (`slot`, `header_hash`, `parent`) |
| `forge.missed_slot` | WARN | A leader slot was not forged; `reason` says why |
| `forge.take_for_forge_failed` | ERROR | Selecting transactions failed; an empty block is forged instead (`slot`, `step`, `error`) |
| `forge.forge_failed` | ERROR | Signing, validating, sealing or storing the block failed (`slot`, `step`, `error`). The node shuts down, and systemd restarts it |

The `reason` of `forge.missed_slot` is one of:

| Reason | Meaning |
|:-------|:--------|
| `ocert_not_yet_valid` | The operational certificate starts in a later KES period than the current one |
| `ocert_expired` | The operational certificate has expired: rotate the KES key |
| `tip_ahead` | The chain was already ahead of the slot to forge |
| `woke_late` | The node woke up too late in the slot, or after it |
| `parent_not_stored` | The parent block for a same-slot lead was not in the chain store |
| `ledger_tip_changed` | The ledger tip changed while the block was being prepared |
| `not_led` | The node woke up for a slot the current schedule does not lead |

After a restart, the node needs some time before `forge.schedule` appears again: seconds on preview, a few minutes on mainnet. Until then it has no schedule to forge from, so restart it when no leader slot is near.

### Leader schedule

`amaru pool schedule inspect` exports the slots your pool leads in an epoch. It reads the node's databases and works while the node runs:

```bash
sudo -u amaru amaru pool schedule inspect \
  --network mainnet \
  --db-chain /var/lib/amaru/chain.mainnet.db \
  --db-ledger /var/lib/amaru/ledger.mainnet.db \
  --operator-vrf /var/lib/amaru/pool/vrf.skey \
  --time-utc
```

The output is JSON with the epoch, the epoch nonce, the pool's stake and share, and the led slots. `--epoch` picks another epoch, as far back as the ledger snapshots kept by the node (`--db-ledger-max-extra-snapshots`). Keep the schedule to yourself: whoever knows your leader slots knows when to disrupt your block producer.

### Metrics

| Metric | Meaning |
|:-------|:--------|
| `cardano_node_metrics_currentKESPeriod_int` | Current KES period |
| `cardano_node_metrics_operationalCertificateStartKESPeriod_int` | KES period at which the operational certificate starts |
| `cardano_node_metrics_operationalCertificateExpiryKESPeriod_int` | KES period at which it expires |
| `cardano_node_metrics_remainingKESPeriods_int` | KES periods left before the KES key must be rotated |

## 6. Rotating the KES key

An operational certificate is valid for 62 KES periods of 129600 slots, about 93 days on mainnet. Rotate well before it expires, and watch `remainingKESPeriods` (one KES period is 36 hours on mainnet).

1. On the **block producer**, create a new KES key pair next to the current one, and read the current KES period:

   ```bash
   sudo -u amaru amaru pool kes create \
     --signing-key /var/lib/amaru/pool/kes.skey.new \
     --verification-key /var/lib/amaru/pool/kes.vkey.new
   curl -s 127.0.0.1:8889/metrics | grep -E '^cardano_node_metrics_currentKESPeriod_int'
   ```

2. Copy `/var/lib/amaru/pool/kes.vkey.new` to the offline machine (the folder is readable by `amaru` only, so use `sudo`) and issue a new certificate there (**offline**), as in [section 3](#3-issuing-the-operational-certificate):

   ```bash
   amaru pool cert create --cert op.cert.new --cold-signing-key cold.skey --counter cold.counter \
     --kes-period <CURRENT_KES_PERIOD> --kes-verification-key kes.vkey.new
   amaru pool cert inspect --cert op.cert.new
   ```

   The `sequence_number` is one higher than the one of the current certificate.

3. Copy `op.cert.new` to the block producer. When no leader slot is near (see `next_slot` in `forge.schedule`), swap the files and restart the node:

   ```bash
   P=/var/lib/amaru/pool
   sudo install -m 0400 -o amaru -g amaru op.cert.new "$P/"
   sudo mv "$P/kes.skey" "$P/kes.skey.old"
   sudo mv "$P/op.cert" "$P/op.cert.old"
   sudo mv "$P/kes.skey.new" "$P/kes.skey"
   sudo mv "$P/op.cert.new" "$P/op.cert"
   sudo chmod 0400 "$P/kes.skey"
   sudo systemctl restart amaru
   ```

   Amaru reads the keys only at startup. There is no reload signal: restart the service.

4. Check that the node uses the new certificate: `build.forging` in the log shows the new `sequence`, `operationalCertificateStartKESPeriod` shows the new start period and `remainingKESPeriods` is back near 62. After the next forged block, a chain explorer shows the new certificate counter for that block. Then delete `kes.skey.old`, `op.cert.old` and `kes.vkey.new` from `/var/lib/amaru/pool`.

To roll back, move the `.old` files back in place of the new ones and restart the node. This works only until the first block with the new certificate is on chain: from then on the node refuses to start with the old one, because its sequence number is lower.
