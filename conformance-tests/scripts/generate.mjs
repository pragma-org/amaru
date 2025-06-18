import * as fs from 'node:fs';
import * as path from 'node:path';
import { bech32 } from 'bech32';
import JsonBig from '@cardanosolutions/json-bigint';

const network = (process.argv[2] ?? "").toLowerCase();

const epoch = Number.parseInt(process.argv[3], 10);

if (Number.isNaN(epoch) ||!["preview", "preprod", "mainnet"].includes(network)) {
  console.log(`Invalid or missing epoch number.

Usage:
    ./generate.mjs <NETWORK> <EPOCH>

Arguments:
    EPOCH     An epoch number as integer
    NETWORK   One of 'preview', 'preprod' or 'mainnet'`);
  process.exit(1);
}

const $ = JsonBig({ useNativeBigInt: true });

const pools = load("pools", epoch);
const nextPools = load("pools", epoch + 1);
const blocks = load("rewards-provenance", epoch + 1);
const distr = load("rewards-provenance", epoch + 3);
const drepsInfo = load("dreps", epoch);
const drepsStake = load("dreps", epoch + 1);
const pots = load("pots", epoch + 3);

const dreps = drepsInfo
  .reduce((accum, drep) => {
    const drepId = toDrepId(drep.id, drep.from, drep.type);

    drep.delegators.forEach((delegator) => {
      accum[delegator.from][delegator.credential] = drepId;
    });

    const stakeInfo = drepsStake.find((future) => drep.id === future.id && drep.from === future.from);

    if (drep.type === "registered") {
      accum.dreps[drepId] = {
        mandate: drep.mandate.epoch,
        metadata: drep.metadata ? ({ url: drep.metadata.url, content_hash: drep.metadata.hash }) : null,
        stake: stakeInfo?.stake.ada.lovelace ?? 0,
      };
    }

    return accum;
  }, {
    verificationKey: {},
    script: {},
    dreps: {
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
    },
  });

// Relative source  of the snapshot test in the target crate.
const source = "crates/amaru-ledger/src/summary/rewards.rs";
const exists = fs.existsSync(`../${source}`);
if (!exists) {
  console.error(`Source file ${source} does not exist.`);
  process.exit(1);
}

// ---------- Rewards summary snapshot

const poolIds = Object.keys(distr.stakePools).sort();

withStream(`summary__stake_distribution_${network}_${epoch}.snap`, (stream) => {
  stream.write("---\n")
  stream.write(`source: ${source}\n`)
  stream.write(`expression: "stake_distr.for_network(Network::Testnet)"\n`)
  stream.write("---\n")
  stream.write("{");
  stream.write(`\n  "epoch": ${epoch},`);
  stream.write(`\n  "active_stake": ${distr.activeStake.ada.lovelace},`);

  const totalVotingStake = Object.values(dreps.dreps).reduce((total, drep) => total + BigInt(drep.stake), 0n);
  stream.write(`\n  "voting_stake": ${totalVotingStake},`);

  let accounts = poolIds.reduce((accum, poolId) => {
    const pool = distr.stakePools[poolId];
    return pool.delegators.reduce((accum, delegator) => {
      const stakeAddress = toStakeAddress(delegator.credential, delegator.from);

      accum[stakeAddress] = {
        lovelace: delegator.stake.ada.lovelace,
	pool: poolId,
        drep: dreps[delegator.from][delegator.credential] ?? null,
      };

      return accum;
    }, accum);
  }, {});
  encodeCollection(stream, "accounts", accounts, false);

  stream.write(`\n  "pools": {\n`)
  poolIds.forEach((k, ix) => {
    const totalStake = BigInt(distr.totalStake.ada.lovelace);
    let [num, den] = (distr.stakePools[k]?.relativeStake || "0/1").split("/");
    den = BigInt(den);

    let stake = 0n;
    if (den === BigInt(distr.totalStake.ada.lovelace)) {
      stake = BigInt(num);
    } else if (num !== "0") {
      stake = BigInt(num) * (totalStake / den);
    }

    const voting_stake = nextPools[k]?.stake.ada.lovelace ?? 0;

    const params = {
      blocks_count: blocks.stakePools[k]?.blocksMade || 0,
      stake,
      voting_stake,
      parameters: {
	id: Buffer.from(bech32.fromWords(bech32.decode(k).words)).toString('hex'),
	vrfVerificationKeyHash: pools[k].vrfVerificationKeyHash,
	pledge: pools[k].pledge,
	cost: pools[k].cost,
	margin: pools[k].margin,
	rewardAccount: pools[k].rewardAccount,
	owners: pools[k].owners,
	relays: pools[k].relays,
	metadata: pools[k].metadata,
      },
    };

    encodeItem(stream, ix, poolIds.length, [k, params]);
  });
  stream.write(",");
  encodeCollection(stream, "dreps", dreps.dreps, true);
  stream.end("\n}");
})

// ---------- Rewards summary snapshots

withStream(`summary__rewards_summary_${network}_${epoch}.snap`, (stream) => {
  stream.write("---\n")
  stream.write(`source: ${source}\n`)
  stream.write(`expression: rewards_summary\n`)
  stream.write("---\n")
  stream.write("{");
  stream.write(`\n  "epoch": ${epoch},`);
  stream.write(`\n  "efficiency": "${distr["efficiency"]}",`);
  stream.write(`\n  "incentives": ${distr["incentives"].ada.lovelace},`);
  stream.write(`\n  "total_rewards": ${distr["totalRewards"].ada.lovelace},`);
  stream.write(`\n  "treasury_tax": ${distr["treasuryTax"].ada.lovelace},`);
  stream.write(`\n  "available_rewards": ${BigInt(distr["totalRewards"].ada.lovelace) - BigInt(distr["treasuryTax"].ada.lovelace)},`);
  stream.write(`\n  "pots": {
    "treasury": ${pots.treasury.ada.lovelace},
    "reserves": ${pots.reserves.ada.lovelace},
    "fees": ${distr["fees"].ada.lovelace}
  },`);
  stream.write(`\n  "pools": {\n`)

  poolIds.forEach((k, ix) => {
    const params = {
      pot: distr.stakePools[k]?.totalRewards.ada.lovelace || 0n,
      leader: distr.stakePools[k]?.leaderReward.ada.lovelace || 0n,
    };
    encodeItem(stream, ix, poolIds.length, [k, params]);
  });
  stream.end("\n}");
});

// ---------- Helpers

function load(dataset, epoch) {
  return $.parse(fs.readFileSync(path.join(import.meta.dirname, "..", "..", "data", network, dataset, `${epoch}.json`)));
}

function withStream(filename, callback) {
  const dir = path.join(import.meta.dirname, "..", "generated");
  fs.mkdirSync(dir, { recursive: true });
  const stream = fs.createWriteStream(path.join(dir, filename));
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
