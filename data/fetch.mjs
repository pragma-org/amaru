import * as fs from "node:fs";
import * as path from "node:path";
import { bech32 } from 'bech32';
import { ogmios, Json } from "@cardano-ogmios/mdk";

const network = (process.argv[2] ?? "").toLowerCase();

const includeSnapshots = (process.argv[3] ?? "false").toLowerCase() == "true";

if (!["preview", "preprod", "mainnet"].includes(network)) {
  console.log(`Missing or invalid network.
Usage:
    ./fetch.mjs <NETWORK> [<INCL_SNAPSHOT_FLAG>]

Arguments:
    NETWORK:  		  One of 'preview', 'preprod' or 'mainnet'
    INCL_SNAPSHOT_FLAG:   A an optional flag (true/false) to also dump fully snapshots listed in configuration.
    			  [default: false]`);
  process.exit(1);
}

const configFile = path.join(import.meta.dirname, network, `config.json`);

const snapshotsDir = path.join(import.meta.dirname, "..", "snapshots", network);
if (includeSnapshots) {
  fs.mkdirSync(snapshotsDir, { recursive: true });
}

// Each point corresponds to the last point of the associated epoch.
const { points, snapshots, additionalStakeAddresses } = JSON.parse(fs.readFileSync(configFile));

const additionalStakeKeys = additionalStakeAddresses.reduce(collectAddressType(14), []);

const additionalStakeScripts = additionalStakeAddresses.reduce(collectAddressType(15), []);

const queries = [
  {
    query: fetchRewardsProvenance,
    getFilename(point) {
      return outDir("rewards-provenance", point);
    },
  },
  {
    query: fetchDReps,
    getFilename(point) {
      return outDir("dreps", point);
    },
  },
  {
    query: fetchPools,
    getFilename(point) {
      return outDir("pools", point);
    },
  },
  {
    query: fetchPots,
    getFilename(point) {
      return outDir("pots", point);
    },
  },
  {
    query: fetchNonces,
    getFilename(point) {
      return outDir("nonces", point);
    },
  },
]

process.stderr.cursorTo(0, 0);
process.stderr.clearScreenDown();

let frame = 0;
const spinner = ["⠋","⠙","⠹","⠸","⠼","⠴","⠦","⠧","⠇","⠏"];
const spinnerId = setInterval(() => {
  process.stderr.cursorTo(0, points.length);
  process.stderr.clearLine(0);
  process.stderr.write(`${spinner[frame]} fetching data${includeSnapshots ? " (incl. snapshots)": ""}`);
  frame = (frame + 1) % spinner.length;
}, 100);

// Run the set of queries for each configured point while the node is
// synchronizing. If a given point isn't available _yet_, pause and
// retry until available.
const tasks = [];
for (let i = 0; i < points.length; i += 1) {
  const tryConnect = async (retry) => {
    const exit = await ogmios((ws, done) => {
      process.stderr.cursorTo(0, i);
      process.stderr.clearLine(0);
      process.stderr.write(`${points[i].slot} => scheduling...`);
      step(ws, i, points[i], done);
    });

    if (exit === undefined) {
      process.stderr.cursorTo(0, i);
      process.stderr.clearLine(0);
      process.stderr.write(`${points[i].slot} => failed to connect; retrying...`);
      await sleep(1000);
      return retry(retry);
    } else {
      return exit;
    }
  };

  tasks.push(tryConnect(tryConnect));
  await sleep(50);
}

const results = await Promise.all(tasks);
clearInterval(spinnerId);
process.stderr.cursorTo(0, points.length);
process.stderr.clearLine(0);

if (!results.every(exit => exit)) {
  console.error(`exited with failures!`);
  console.log(results);
  process.exit(1);
}

async function sleep(ms) {
  await new Promise(resolve => setTimeout(resolve, ms));
}

function step(ws, i, point, done) {
  ws.once("message", async (data) => {
    process.stderr.cursorTo(0, i);

    const { error } = Json.parse(data);

    if (error) {
      if (error.code !== 2000 || !/doesn't or no longer exist/.test(error.data)) {
    	process.stderr.clearLine(0);
        process.stderr.write(`${point.slot} => [error ${error.code}] ${error.message} (${error.data})`);
	return done(false);
      }

      process.stderr.write(`${point.slot} => not available yet...`);
      process.stderr.cursorTo(0, i);
      return setTimeout(() => step(ws, i, point, done), 500);
    }

    process.stderr.clearLine(0);
    process.stderr.write(`${point.slot} => querying...`);

    if (includeSnapshots && snapshots.includes(point.epoch)) {
      const to = path.join(snapshotsDir, `${point.slot}.${point.id}.cbor`);
      await ws.queryLedgerState("dump", { to });
    }

    let result;
    for (let q = 0; q < queries.length; q += 1) {
      const { query, getFilename } = queries[q];
      const filename = getFilename(point);
      fs.mkdirSync(path.dirname(filename), { recursive: true });
      result = await query(ws, result)
      fs.writeFileSync(filename, Json.stringify(result, null, 2));
    }

    process.stderr.cursorTo(0, i);
    process.stderr.clearLine(0);
    process.stderr.write(`${point.slot} => ✓`);

    done(true);
  });

  ws.rpc("acquireLedgerState", { point });
}

function decodeBech32(str) {
  return Buffer.from(bech32.fromWords(bech32.decode(str).words));
}

function collectAddressType(addressType) {
  return (accum, addr) => {
    const bytes = decodeBech32(addr);

    if ((bytes[0] >> 4) == addressType) {
      accum.push(bytes.slice(1).toString('hex'));
    }

    return accum;
  }
}

function outDir(prefix, point) {
  return path.join(import.meta.dirname, network, prefix, `${point.epoch}.json`);
}

function fetchRewardsProvenance(ws) {
  return ws.queryLedgerState("rewardsProvenance")
}

async function fetchDReps(ws, { stakePools }) {
  let dreps = await ws.queryLedgerState("delegateRepresentatives");

  let abstain = dreps.find((drep) => drep.type === "abstain") ?? { stake: { ada: { lovelace: 0 } } };
  abstain.delegators = [];

  let noConfidence = dreps.find((drep) => drep.type === "noConfidence") ?? { stake: { ada: { lovelace: 0 } } };
  noConfidence.delegators = [];

  // TODO: Fix Ledger-State Query protocol...
  //
  // 'abstain' and 'noConfidence' do not contain their delegators. So we do a
  // best attempt at resolving them. We can't easily obtain a list of all
  // registered stake account either; so we instead use all the
  // delegators we know to pools and look amongst them.
  //
  // We add 'additionalStakeKeys' and 'additionalStakeScripts' for those
  // delegators that would be:
  //
  // a. Not delegating to any pool
  // b. Delegating to an abstain or noConfidence drep
  //
  // These additions are very much empiric; they will first manifest as a
  // snapshot mismatch when we test. The generated snapshots will
  // indicate a `null` drep, whereas Amaru would yield `abstain` or
  // `noConfidence`.
  //
  // Note that this is far from ideal, because cases where we (amaru) fail to
  // correctly identify a delegation will simply go unnoticed. But this
  // isn't easily fixable without altering the state query client
  // protocol at the node's level -- or, by resorting to using a debug
  // new epoch state snapshot.
  let { verificationKey: keys, script: scripts } = Object.keys(stakePools).reduce((accum, pool) => {
    stakePools[pool].delegators.forEach((delegator) => {
       accum[delegator.from].add(delegator.credential);
    });

    return accum;
  }, { verificationKey: new Set(), script: new Set() });

  const drepsMap = dreps.reduce((accum, drep) => {
    drep.delegators.forEach((delegator) => {
      if (delegator.from === "verificationKey") {
	keys.add(delegator.credential);
      } else {
	scripts.add(delegator.credential);
      }
    });

    drep.delegators = [];

    if (drep.type === "registered") {
      accum[drep.from][drep.id] = drep;
    }

    return accum;
  }, { verificationKey: {}, script: {} });


  const summaries = await ws.queryLedgerState("rewardAccountSummaries", {
    keys: Array.from(keys).concat(additionalStakeKeys),
    scripts: Array.from(scripts).concat(additionalStakeScripts),
  });

  // There's also a bug in the ledger where the DRep state may still contain
  // now-removed delegations. So we cannot trust the data coming from the
  // `delegateRepresentatives` endpoint when it comes to delegators
  // (as it's always a superset).
  //
  // But we can trust the one from 'rewardAccountSummaries'. So we aren't only
  // recovering delegators for abstain and no-confidence roles, but also repairing
  // the drep delegators altogether.
  summaries.forEach(({ from, credential, delegateRepresentative: drep }) => {
    if (drep?.type === "abstain") {
      abstain.delegators.push({ from, credential });
    } else if (drep?.type === "noConfidence") {
      noConfidence.delegators.push({ from, credential });
    } else if (drep) {
      drepsMap[drep.from][drep.id]?.delegators.push({ from, credential });
    }
  });

  return dreps;
}

function fetchPots(ws) {
  return ws.queryLedgerState("treasuryAndReserves")
}

function fetchPools(ws) {
  return ws.queryLedgerState("stakePools", { includeStake: true });
}

function fetchNonces(ws) {
  return ws.queryLedgerState("nonces");
}
