// Copyright 2025 PRAGMA
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

import * as fs from 'node:fs';
import * as path from 'node:path';
import { bech32 } from 'bech32';
import JsonBig from '@cardanosolutions/json-bigint';

const network = (process.argv[2] ?? "").toLowerCase();
const epoch = Number.parseInt(process.argv[3], 10);

if (Number.isNaN(epoch) || !["preview", "preprod", "mainnet"].includes(network)) {
  console.log(`Invalid or missing epoch number.

Usage:
    ./generate.mjs <NETWORK> <EPOCH>

Arguments:
    EPOCH     An epoch number as integer
    NETWORK   One of 'preview', 'preprod' or 'mainnet'`);
  process.exit(1);
}

const $ = JsonBig({ useNativeBigInt: true });

// Load conformance data in new format (snake_case, plain numbers)
const pools = load("pools", epoch);
const nextPools = load("pools", epoch + 1);
const blocks = load("rewards-provenance", epoch + 1);
const distr = load("rewards-provenance", epoch + 3);
const drepsInfo = load("dreps", epoch);
const drepsStake = load("dreps", epoch + 1);
const pots = load("pots", epoch + 3);

const dreps = drepsInfo.reduce((accum, drep) => {
  const drepId = toDrepId(drep.hash, drep.from, drep.type);

  if (drep.mandate === undefined && drepId != "abstain" && drepId != "no_confidence") {
    return accum;
  }

  drep.delegators.forEach((delegator) => {
    const bucket = delegator.from;
    if (accum[bucket] !== undefined) {
      accum[bucket][delegator.hash] = drepId;
    }
  });

  const stakeInfo = drepsStake.find((future) => future.hash === drep.hash && future.from === drep.from);

  if (drep.type === "registered" && drep.mandate !== undefined) {
    accum.dreps[drepId] = {
      mandate: drep.mandate.epoch,
      metadata: drep.metadata ? ({ url: drep.metadata.url, content_hash: drep.metadata.hash }) : null,
      stake: BigInt(stakeInfo?.stake ?? 0),
    };
  }

  return accum;
}, {
  verification_key: {},
  script: {},
  dreps: {
    abstain: {
      mandate: null,
      metadata: null,
      stake: BigInt(drepsStake.find((future) => future.type === "abstain")?.stake ?? 0),
    },
    no_confidence: {
      mandate: null,
      metadata: null,
      stake: BigInt(drepsStake.find((future) => future.type === "noConfidence")?.stake ?? 0),
    },
  },
});

const source = "crates/amaru/tests/summary.rs";
const repoRoot = path.join(import.meta.dirname, "..", "..");
const sourcePath = path.join(repoRoot, source);
const exists = fs.existsSync(sourcePath);
if (!exists) {
  console.error(`Source file ${source} does not exist at ${sourcePath}`);
  process.exit(1);
}

const poolIds = Object.keys(distr.stake_pools).sort();

// Generate stake_distribution snapshot
withStream(`summary__stake_distribution_${epoch}.snap`, (stream) => {
  stream.write("---\n");
  stream.write(`source: ${source}\n`);
  stream.write(`assertion_line: 100\n`);
  stream.write(`expression: "stake_distr.for_network(network.into())"\n`);
  stream.write("---\n");
  stream.write("{");
  stream.write(`\n  "epoch": ${epoch},`);
  stream.write(`\n  "active_stake": ${distr.active_stake},`);

  const totalVotingStake = Object.values(dreps.dreps).reduce((total, drep) => total + BigInt(drep.stake), 0n);
  stream.write(`\n  "voting_stake": ${totalVotingStake},`);

  let accounts = poolIds.reduce((accum, poolId) => {
    const pool = distr.stake_pools[poolId];
    return pool.delegators.reduce((accum, delegator) => {
      const stakeAddress = toStakeAddress(delegator.hash, delegator.from);
      const drep = dreps[delegator.from]?.[delegator.hash] ?? null;

      accum[stakeAddress] = {
        lovelace: BigInt(delegator.stake),
        pool: toPoolId(poolId),
        drep,
      };

      return accum;
    }, accum);
  }, {});
  encodeCollection(stream, "accounts", accounts, false);

  stream.write(`\n  "pools": {\n`);
  poolIds.forEach((k, ix) => {
    const totalStake = BigInt(distr.total_stake);
    const [num, den] = distr.stake_pools[k].relative_stake;
    const numerator = BigInt(num);
    const denominator = BigInt(den);

    let stake = 0n;
    if (denominator === totalStake) {
      stake = numerator;
    } else if (numerator !== 0n) {
      stake = numerator * (totalStake / denominator);
    }

    const voting_stake = nextPools[k]?.stake ?? 0;

    const params = {
      blocks_count: blocks.stake_pools[k]?.blocks_made || 0,
      stake,
      voting_stake,
      parameters: {
        id: k,
        vrfVerificationKeyHash: pools[k].vrf,
        pledge: pools[k].pledge,
        cost: pools[k].cost,
        margin: pools[k].margin,
        rewardAccount: pools[k].reward_account,
        owners: pools[k].owners,
        relays: pools[k].relays,
        metadata: pools[k].metadata,
      },
    };

    encodeItem(stream, ix, poolIds.length, [toPoolId(k), params]);
  });
  stream.write(",");
  encodeCollection(stream, "dreps", dreps.dreps, true);
  stream.end("\n}");
});

// Generate rewards_summary snapshot
withStream(`summary__rewards_summary_${epoch}.snap`, (stream) => {
  stream.write("---\n");
  stream.write(`source: ${source}\n`);
  stream.write(`expression: rewards_summary\n`);
  stream.write("---\n");
  stream.write("{");
  stream.write(`\n  "epoch": ${epoch},`);
  stream.write(`\n  "efficiency": "${distr.efficiency}",`);
  stream.write(`\n  "incentives": ${distr.incentives},`);
  stream.write(`\n  "total_rewards": ${distr.total_rewards},`);
  stream.write(`\n  "treasury_tax": ${distr.treasury_tax},`);
  stream.write(`\n  "available_rewards": ${BigInt(distr.total_rewards) - BigInt(distr.treasury_tax)},`);
  stream.write(`\n  "pots": {
    "treasury": ${pots.treasury},
    "reserves": ${pots.reserves},
    "fees": ${distr.fees}
  },`);
  stream.write(`\n  "pools": {\n`);

  poolIds.forEach((k, ix) => {
    const params = {
      pot: distr.stake_pools[k]?.total_rewards || 0n,
      leader: distr.stake_pools[k]?.leader_reward || 0n,
    };
    encodeItem(stream, ix, poolIds.length, [toPoolId(k), params]);
  });
  stream.end("\n}");
});

// ===== Helpers =====

function load(dataset, epoch) {
  // Load from conformance archive (new format only)
  const conformanceDir = path.join(import.meta.dirname, "..", "..", "snapshots", network, `conformance-${network}-${epoch}`);
  const conformancePath = path.join(conformanceDir, dataset, `${epoch}.json`);
  
  try {
    return $.parse(fs.readFileSync(conformancePath));
  } catch (err) {
    console.error(`Failed to load ${dataset} for epoch ${epoch} from ${conformancePath}`);
    console.error(`Ensure conformance archives are extracted to snapshots/${network}/`);
    process.exit(1);
  }
}

function withStream(filename, callback) {
  const dir = path.join(import.meta.dirname, "..", "..", "crates", "amaru", "tests", "snapshots", network);
  fs.mkdirSync(dir, { recursive: true });
  const stream = fs.createWriteStream(path.join(dir, filename));
  callback(stream);
}

// CIP-0129: DRep ID encoding
function toDrepId(hash, category, type) {
  if (type === "abstain") return "abstain";
  if (type === "noConfidence") return "no_confidence";
  const isKey = category === "verification_key";
  return bech32.encode(
    "drep",
    bech32.toWords(
      Buffer.concat([
        Buffer.from([isKey ? 34 : 35]),
        Buffer.from(hash, "hex"),
      ])
    )
  );
}

// Pool ID encoding (hex to bech32)
function toPoolId(hexId) {
  if (hexId.startsWith("pool1")) return hexId;
  return bech32.encode(
    "pool",
    bech32.toWords(Buffer.from(hexId, "hex"))
  );
}

function toStakeAddress(hash, category) {
  const isKey = category === "verification_key";
  const prefix = network === "mainnet"
    ? (isKey ? 0xe1 : 0xf1)
    : (isKey ? 0xe0 : 0xf0);
  const hrp = network === "mainnet" ? "stake" : "stake_test";

  return bech32.encode(
    hrp,
    bech32.toWords(
      Buffer.concat([
        Buffer.from([prefix]),
        Buffer.from(hash, "hex"),
      ])
    )
  );
}

function encodeItem(stream, ix, maxItems, [k, v], isLast = true, indent = 4) {
  const pad = "".padEnd(indent, " ");
  const padEnd = "".padEnd(indent - 2, " ");
  const json = $.stringify(v, null, 2);
  const row = json
    .split("\n")
    .map(x => `${pad}${x}`)
    .join("\n")
    .slice(indent);
  stream.write(`${pad}"${k}": ${row}`);
  if (ix == maxItems - 1) {
    stream.write(`\n${padEnd}}${isLast ? '' : ','}`);
  } else {
    stream.write(',\n');
  }
}

function encodeCollection(stream, name, items, isLast = true, indent = 2) {
  const pad = "".padEnd(indent, " ");
  const keys = Object.keys(items).sort();
  stream.write(`\n${pad}"${name}": {${keys.length > 0 ? '\n' : ''}`);
  keys.forEach((k, ix) => {
    encodeItem(stream, ix, keys.length, [k, items[k]], isLast, indent + 2);
  });
  if (keys.length === 0) {
    stream.write(`}${isLast ? '' : ','}`);
  }
}
