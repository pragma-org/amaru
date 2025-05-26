import * as fs from "node:fs";
import * as path from "node:path";
import { ogmios, Json } from "@cardano-ogmios/mdk";

const network = process.env.NETWORK;
if (!network) {
  console.log(`Missing network.

Usage:
    ./fetch.mjs <NETWORK>`);
  process.exit(1);
}
// Each point corresponds to the last point of the associated epoch.
const { points, additionalStakeAddresses } = JSON.parse(fs.readFileSync(path.join(import.meta.dirname, "..", `config/${network}.json`)));

function outDir(prefix, point) {
  return path.join(import.meta.dirname, "..", "data", prefix, `${point.epoch}.json`);
}

const queries = [
  {
    query: fetchPools,
    getFilename(point) {
      return outDir("pools", point);
    },
  },
  {
    query: fetchDReps,
    getFilename(point) {
      return outDir("dreps", point);
    },
  },
  {
    query: fetchDRepsDelegations,
    getFilename(point) {
      return outDir("dreps-delegations", point);
    },
  },
  {
    query: fetchPots,
    getFilename(point) {
      return outDir("pots", point);
    },
  },
    {
    query: fetchNewEpochState,
    getFilename(point) {
      return outDir("new-epoch-state", point);
    },
  },
  {
    query: fetchNonces,
    getFilename(point) {
      return outDir("nonces", point);
    },
  },
]

const result = await ogmios(async (ws, done) => {
  let lastPrintedError;

  function step(point, next) {
    ws.once("message", async (data) => {
      const { error } = Json.parse(data);

      if (error) {
        if (error.code !== 2000 || !/doesn't or no longer exist/.test(error.data)) {
          console.error(`[error ${error.code}] ${error.message} (${error.data})`);
          return process.exit(1);
        }

        const feedback = `${point.slot} => ${error.data}`;

        if (lastPrintedError !== undefined && lastPrintedError !== feedback) {
          process.stderr.moveCursor(0, -1)
          process.stderr.clearLine(1)
        }

        if (lastPrintedError !== feedback) {
          console.error(feedback);
          lastPrintedError = feedback;
        }

        return setTimeout(() => step(point, next), 200)
      }

      for (let q = 0; q < queries.length; q += 1) {
        const { query, getFilename } = queries[q];
        const filename = getFilename(point);
        fs.mkdirSync(path.dirname(filename), { recursive: true });
        fs.writeFileSync(filename, Json.stringify(await query(ws), null, 2));
      }

      next();
    });

    ws.rpc("acquireLedgerState", { point });
  }

  // Run the set of queries for each configured point while the node is
  // synchronizing. If a given point isn't available _yet_, pause and
  // retry until available.
  for (let i = 0; i < points.length; i += 1) {
    await new Promise((next) => step(points[i], next));
  }

  done(true);
});

if (!result) {
  console.error(`exited without completing task; is the node running?`);
  process.exit(1);
}

function fetchPools(ws) {
  return ws.queryLedgerState("stakePools", { includeStake: true });
}

function fetchDReps(ws) {
  return ws.queryLedgerState("delegateRepresentatives");
}

async function fetchDRepsDelegations(ws) {
  const dreps = await fetchDReps(ws);

  const { keys, scripts } = dreps.reduce((accum, drep) => {
    drep.delegators?.forEach((delegator) => {
      if (delegator.from === "verificationKey") {
        accum.keys.add(delegator.credential);
      } else {
        accum.scripts.add(delegator.credential);
      }
    });
    return accum;
  }, { keys: new Set(), scripts: new Set() });

  const summaries = await ws.queryLedgerState("rewardAccountSummaries", {
    keys: Array.from(keys).concat(additionalStakeAddresses),
    scripts: Array.from(scripts),
  });

  return Object.keys(summaries).reduce((accum, k) => {
    const summary = summaries[k].delegateRepresentative;
    if (summary !== undefined) {
      accum[k] = summary;
    }
    return accum;
  }, {});
}

function fetchPots(ws) {
  return ws.queryLedgerState("treasuryAndReserves")
}

function fetchNewEpochState(ws) {
  return ws.queryLedgerState("dump");
}

function fetchNonces(ws) {
  return ws.queryLedgerState("nonces");
}