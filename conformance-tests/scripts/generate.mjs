import * as fs from 'node:fs';
import * as path from 'node:path';
import { bech32 } from 'bech32';
import JsonBig from '@cardanosolutions/json-bigint';

const epoch = Number.parseInt(process.argv[2], 10);

if (Number.isNaN(epoch)) {
  console.log(`Invalid or missing epoch number.

Usage:
    ./generate.js <EPOCH>`);
  process.exit(1);
}

const $ = JsonBig({ useNativeBigInt: true });

const DREP_TYPES = {
  "noConfidence": "no_confidence",
  "abstain": "abstain",
  "registered": "registered",
};

const { additionalStakeAddresses } = loadConfig();
const pools = load("pools", epoch + 1);
const epochState = load("epoch-state", epoch + 1);
const blocks = load("rewards-provenance", epoch + 1);
const distr = load("rewards-provenance", epoch + 3);
const drepsDelegations = load("dreps-delegations", epoch);
const drepsInfo = load("dreps", epoch);
const drepsStake = load("dreps", epoch + 1);
const pots = load("pots", epoch + 3);
const dreps = Object.keys(drepsDelegations)
  .reduce((acc, credential) => {
    const drep = drepsDelegations[credential];

    const drepId = toDrepId(drep.id, drep.from, drep.type);

    if (drep.type !== "registered") {
      const isKey = additionalStakeAddresses.includes(toStakeAddress(credential, "verificationKey"));
      const isScript = additionalStakeAddresses.includes(toStakeAddress(credential, "script"));

      if (isKey && !isScript) {
        acc.keys[credential] = drepId;
      }

      if (!isKey && isScript) {
        acc.scripts[credential] = drepId;
      }

      if (isKey && isScript) {
        throw `credential ${credential} is too ambiguious; exists as both key and script!`;
      }

      if (!isKey && !isScript) {
        throw `unexpected unknown credential ${credential}`;
      }

      return acc;
    }

    const info = drepsInfo.find(({ id, from }) => id == drep.id && from == drep.from);

    if (info === undefined) {
      return acc;
    }

    // TODO: Also add the values of the abstain / no-confidence dreps somewhere?
    if (drep.type === "registered") {
      const category = info.delegators.find((deleg) => deleg.credential === credential).from === "verificationKey"
        ? "keys"
        : "scripts";
      acc[category][credential] = drepId;
    }

    return acc;
  }, {
    keys: {},
    scripts: {},
    dreps: drepsInfo.reduce((acc, drep) => {
      const drepId = toDrepId(drep.id, drep.from, drep.type);

      const stakeInfo = drepsStake.find((future) => drep.id === future.id && drep.from === future.from);

      if (drep.type === "registered") {
        acc[drepId] = {
          mandate: drep.mandate.epoch,
          metadata: drep.metadata ? ({ url: drep.metadata.url, content_hash: drep.metadata.hash }) : null,
	  stake: stakeInfo?.stake.ada.lovelace ?? 0,
        };
      }

      return acc;
    }, {
      abstain: {
	mandate: null,
	metadata: null,
	stake: drepsStake.find((future) => future.type === "abstain")?.stake.ada.lovelace ?? 0,
      },
      no_confidence: {
	mandate: null,
	metadata: null,
	stake: drepsStake.find((future) => future.type === "noConfidence")?.stake.ada.lovelace ?? 0,
      },
    }),
  });

// Relative source  of the snapshot test in the target crate.
const source = "crates/amaru/src/ledger/rewards.rs";

// ---------- Rewards summary snapshot

const poolsParams = Object.keys(epochState.stakePoolParameters).sort();
withStream(`rewards__stake_distribution_${epoch}.snap`, (stream) => {
  stream.write("---\n")
  stream.write(`source: ${source}\n`)
  stream.write(`expression: stake_distr\n`)
  stream.write("---\n")
  stream.write("{");
  stream.write(`\n  "epoch": ${epoch},`);
  stream.write(`\n  "active_stake": ${distr.activeStake},`);

  const totalVotingStake = Object.values(dreps.dreps).reduce((total, drep) => total + BigInt(drep.stake), 0n);
  stream.write(`\n  "voting_stake": ${totalVotingStake},`);

  let accounts = {}
  Object.keys(epochState.keys)
    .reduce((accum, key) => {
      accum[toStakeAddress(key, "verificationKey")] = { ...epochState.keys[key], drep: dreps.keys[key] ?? null };
      return accum;
    }, accounts);
  Object.keys(epochState.scripts)
    .reduce((accum, script) => {
      accum[toStakeAddress(script, "script")] = { ...epochState.scripts[script], drep: dreps.scripts[script] ?? null };
      return accum;
    }, accounts);
  encodeCollection(stream, "accounts", accounts, false);

  stream.write(`\n  "pools": {\n`)
  poolsParams.forEach((k, ix) => {
    const totalStake = BigInt(distr.totalStake);
    let [num, den] = (distr.pools[k]?.relativeStake || "0/1").split("/");
    den = BigInt(den);

    let stake = 0n;
    if (den === distr.totalStake) {
      stake = BigInt(num);
    } else if (num !== "0") {
      stake = BigInt(num) * (totalStake / den);
    }

    const voting_stake = pools[k]?.stake.ada.lovelace ?? 0;

    const params = {
      blocks_count: blocks.pools[k]?.blocksMade || 0,
      stake,
      voting_stake,
      parameters: epochState.stakePoolParameters[k],
    };

    encodeItem(stream, ix, poolsParams.length, [k, params]);
  });
  stream.write(",");
  encodeCollection(stream, "dreps", dreps.dreps, true);
  stream.end("\n}");
})

// ---------- Rewards summary snapshots

withStream(`rewards__rewards_summary_${epoch}.snap`, (stream) => {
  stream.write("---\n")
  stream.write(`source: ${source}\n`)
  stream.write(`expression: rewards_summary\n`)
  stream.write("---\n")
  stream.write("{");
  stream.write(`\n  "epoch": ${epoch},`);
  stream.write(`\n  "efficiency": "${distr["η"]}",`);
  stream.write(`\n  "incentives": ${distr["ΔR1"]},`);
  stream.write(`\n  "total_rewards": ${distr["rewardPot"]},`);
  stream.write(`\n  "treasury_tax": ${distr["ΔT1"]},`);
  stream.write(`\n  "available_rewards": ${distr["rewardPot"] - distr["ΔT1"]},`);
  stream.write(`\n  "pots": {
    "treasury": ${pots.treasury.ada.lovelace},
    "reserves": ${pots.reserves.ada.lovelace},
    "fees": ${distr["rewardPot"] - distr["ΔR1"]}
  },`);
  stream.write(`\n  "pools": {\n`)
  poolsParams.forEach((k, ix) => {
    const params = {
      pot: distr.pools[k]?.rewardPot || 0n,
      leader: distr.pools[k]?.leaderReward || 0n,
    };
    encodeItem(stream, ix, poolsParams.length, [k, params]);
  });
  stream.end("\n}");
});

// ---------- Helpers

function loadConfig() {
  return $.parse(fs.readFileSync(path.join(import.meta.dirname, "..", "config.json")));
}

function load(dataset, epoch) {
  return $.parse(fs.readFileSync(path.join(import.meta.dirname, "..", "data", dataset, `${epoch}.json`)));
}

function withStream(filename, callback) {
  const stream = fs.createWriteStream(path.join(import.meta.dirname, "..", "snapshots", filename));
  callback(stream);
  console.log(`✓ ${path.relative(path.join(import.meta.dirname, ".."), stream.path)}`);
}

// As per CIP-0129
function toDrepId(str, category, type) {
  if (type === "abstain") { return "abstain"; }
  if (type === "noConfidence") { return "no_confidence"; }
  return bech32.encode(
    "drep",
    bech32.toWords(
      Buffer.concat([
        Buffer.from([category === "verificationKey" ? 34 : 35]),
        Buffer.from(str, "hex"),
      ])
    )
  );
}

function toStakeAddress(str, category) {
  return bech32.encode(
    "stake_test",
    bech32.toWords(
      Buffer.concat([
        Buffer.from([category === "verificationKey" ? 0xe0 : 0xf0]),
        Buffer.from(str, "hex"),
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
  stream.write(`\n${pad}"${name}": {${keys.length > 0 ? '\n' : ''}`)
  keys.forEach((k, ix) => {
    encodeItem(stream, ix, keys.length, [k, items[k]], isLast, indent + 2);
  });
  if (keys.length === 0) {
    stream.write(`}${isLast ? '' : ','}`);
  }
}
