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

import * as fs from "node:fs";
import * as path from "node:path";
import { bech32 } from 'bech32';
import { ogmios, Json } from "@cardano-ogmios/mdk";

const spinner = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

const network = (process.argv[2] ?? "").toLowerCase();

const includeSnapshots = (process.argv[3] ?? "false").toLowerCase() == "true";

const ogmiosUrl = process.env.OGMIOS_URL ?? "ws://127.0.0.1:1337";

if (!["preview", "preprod", "mainnet", "custom"].includes(network)) {
  console.log(`Missing or invalid network.
Usage:
    ./fetch.mjs <NETWORK> [<INCL_SNAPSHOT_FLAG>]

Arguments:
    NETWORK:              One of 'preview', 'preprod', 'mainnet' or 'custom'
    INCL_SNAPSHOT_FLAG:   A an optional flag (true/false) to also dump fully snapshots listed in configuration.
                          [default: false]`);
  process.exit(1);
}

// Check whether stderr is a tty before trying fancy stuff
const tty = {
  ok: process.stderr.isTTY && typeof process.stderr.cursorTo === "function",
  cursorTo: (...args) => tty.ok && process.stderr.cursorTo(...args),
  clearLine: (...args) => tty.ok && process.stderr.clearLine(...args),
  clearScreenDown: () => tty.ok && process.stderr.clearScreenDown(),
  write: (s) => tty.ok ? process.stderr.write(s) : console.error(s),
};

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

const snapshotsDir = path.join(import.meta.dirname, "..", "snapshots", network);
if (includeSnapshots) {
  fs.mkdirSync(snapshotsDir, { recursive: true });
}

let additionalStakeKeys = [];
let additionalStakeScripts = [];

const configFile = path.join(import.meta.dirname, `../crates/amaru/config/bootstrap/${network}`, `config.json`);

tty.cursorTo(0, 0);
tty.clearScreenDown();

if (fs.existsSync(configFile)) {
  // Each point corresponds to the last point of the associated epoch.
  const { points: configPoints, snapshots, additionalStakeAddresses } = JSON.parse(fs.readFileSync(configFile));
  if (!snapshots || !Array.isArray(snapshots)) {
    console.error(`Invalid or missing snapshots in ${configFile}`);
    process.exit(1);
  }

  const existingPoints = filterExistingPoints(network);

  const points = configPoints.filter(point => !existingPoints.has(point.epoch));

  additionalStakeKeys = additionalStakeAddresses.reduce(collectAddressType(14), []);

  additionalStakeScripts = additionalStakeAddresses.reduce(collectAddressType(15), []);

  await fetchSpecificPoints(points, snapshots, additionalStakeKeys, additionalStakeScripts);
} else {
  const run = async (retry) => {
    try {
      await retry();
    } catch (e) {
      console.log(e);
      await run(retry);
    }
  };

  await run(fetchContinuously);
}

async function fetchContinuously() {
  const tryConnect = async (retry) => {
    const exit = await ogmios(async (ws, done) => {
      tty.write(`connected to ogmios(${ogmiosUrl})`);

      const eraSummaries = await ws.queryLedgerState("eraSummaries");

      const genesisParameters = await ws.queryNetwork("genesisConfiguration", {
        "era": "shelley"
      });

      const networkEpoch = currentEpoch(eraSummaries, genesisParameters);

      let frame = 0;
      const spinnerId = setInterval(() =>
        {
          if (tty.ok) {
            tty.cursorTo(0, 0);
            tty.clearLine(0);
            tty.write(`${spinner[frame]} fetching data until epoch=${networkEpoch}`);
            frame = (frame + 1) % spinner.length;
          }
        }, 100);

      const ledgerTip = await ws.queryLedgerState("tip");

      const chainFollower = await ws.newChainFollower([ledgerTip]);

      let previousTip = null;
      for await (const { block } of chainFollower()) {
        const point = {
          id: block.id,
          slot: block.slot,
          epoch: slotToEpoch(eraSummaries, block.slot)
        };

        if (previousTip != null) {
          tty.cursorTo(0, 1);
          tty.clearLine(0);
          const num = relativeSlot(eraSummaries, point.slot);
          const den = epochLength(eraSummaries, point.slot);
          tty.write(`awaiting end of epoch ${previousTip.epoch} (${num}/${den})`);
        }

        if (previousTip != null && point.epoch > previousTip.epoch) {
          // Run the step through a different socket, so that messages don't conflict.
          await ogmios(async (ws, done) => {
            step(ws, [], 1, previousTip, done);
          });
        }

        if (point.epoch >= networkEpoch) {
          break;
        }

        previousTip = point;
      }

      clearInterval(spinnerId);

      done(true);
    }, ogmiosUrl);

    if (exit === undefined) {
      tty.cursorTo(0, 0);
      tty.clearLine(0);
      tty.write(`failed to connect; retrying...`);
      await sleep(1000);
      return retry(retry);
    } else {
      tty.cursorTo(0, 0);
      tty.clearScreenDown();
      return exit;
    }
  };

  await tryConnect(tryConnect);
}

// Fetch specific points from a remote, allowing for parallel fetching in case
// synchronization is faster.
async function fetchSpecificPoints(points, snapshots, additionalStakeKeys, additionalStakeScripts) {
  function andThen(ws, done) {
    return (ok, i) => {
      if (ok && points[i + 10] !== undefined) {
        return step(ws, snapshots, i + 10, points[i + 10], andThen(ws, done));
      }

      return done(ok);
    };
  }

  let frame = 0;
  const spinnerId = setInterval(() => {
    if (tty.ok) {
      tty.cursorTo(0, Math.min(10, points.length));
      tty.clearLine(0);
      tty.write(`${spinner[frame]} fetching data${includeSnapshots ? " (incl. snapshots)" : ""}`);
      frame = (frame + 1) % spinner.length;
    }
  }, 100);

  // Run the set of queries for each configured point while the node is
  // synchronizing. If a given point isn't available _yet_, pause and
  // retry until available.
  const tasks = [];
  for (let i = 0; i < Math.min(10, points.length); i += 1) {
    const tryConnect = async (retry) => {
      const exit = await ogmios((ws, done) => {
        tty.cursorTo(0, i);
        tty.clearLine(0);
        tty.write(`${points[i].slot} => scheduling...`);
        step(ws, snapshots, i, points[i], andThen(ws, done));
      }, ogmiosUrl);

      if (exit === undefined) {
        tty.cursorTo(0, i);
        tty.clearLine(0);
        tty.write(`${points[i].slot} => failed to connect; retrying...`);
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
  tty.cursorTo(0, Math.min(10, points.length));
  tty.clearLine(0);

  if (!results.every(exit => exit)) {
    console.error(`exited with failures!`);
    console.log(results);
    process.exit(1);
  }
}

function step(ws, snapshots, i, point, done) {
  ws.once("message", async (data) => {
    tty.cursorTo(0, i % 10);

    const { error } = Json.parse(data);

    if (error) {
      if (error.code !== 2000 || !/doesn't or no longer exist/.test(error.data)) {
        tty.clearLine(0);
        tty.write(`${point.slot} => [error ${error.code}] ${error.message} (${error.data})`);
        return done(false, i);
      }

      tty.write(`${point.slot} => not available yet...`);
      tty.cursorTo(0, i % 10);
      return setTimeout(() => step(ws, snapshots, i, point, done), 500);
    }

    tty.clearLine(0);
    tty.write(`${point.slot} => querying...`);

    if (includeSnapshots && snapshots.includes(point.epoch)) {
      const to = path.join(snapshotsDir, `${point.slot}.${point.id}.cbor`);
      await ws.queryLedgerState("dump", { to });
    }

    for (let q = 0; q < queries.length; q += 1) {
      const { query, getFilename } = queries[q];
      const filename = getFilename(point);
      fs.mkdirSync(path.dirname(filename), { recursive: true });
      const result = await query(ws);
      fs.writeFileSync(filename, Json.stringify(result));
    }

    tty.cursorTo(0, i % 10);
    tty.clearLine(0);
    tty.write(`${point.slot} => ✓`);

    done(true, i);
  });

  ws.rpc("acquireLedgerState", { point });
}

async function sleep(ms) {
  await new Promise(resolve => setTimeout(resolve, ms));
}

function filterExistingPoints(network) {
  const folderPath = path.join(import.meta.dirname, network, "dreps");

  if (!fs.existsSync(folderPath)) {
    return new Set();
  }

  const files = fs.readdirSync(folderPath);

  return new Set(
    files
      .filter(file => file.endsWith('.json'))
      .map(file => parseInt(path.basename(file, '.json')))
      .filter(num => !isNaN(num))
  );
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
  };
}

function outDir(prefix, point) {
  return path.join(import.meta.dirname, network, prefix, `${point.epoch}.json`);
}

function fetchRewardsProvenance(ws) {
  return ws.queryLedgerState("rewardsProvenance");
}

async function fetchDReps(ws, stakePools = {}) {
  return ws.queryLedgerState("delegateRepresentatives");
}

function findEra(eraSummaries, slot) {
  let era = eraSummaries.findLast(() => true);

  for (const summary of eraSummaries) {
    if (summary.end !== undefined && summary.end?.slot < slot) {
      continue;
    }

    if (slot >= summary.start.slot) {
      era = summary;
    }
  }

  return era;
}

function currentEpoch(eraSummaries, genesisParameters) {
  const lastEra = eraSummaries.findLast(() => true);
  const now = Date.now();
  const start = new Date(genesisParameters.startTime);
  const deltaSlot = Math.floor((now - start.getTime() - 1000 * lastEra.start.time.seconds)/1000);
  return lastEra.start.epoch + Math.floor(deltaSlot / lastEra.parameters.epochLength);
}

function relativeSlot(eraSummaries, slot) {
  const era = findEra(eraSummaries, slot);
  return (slot - era.start.slot) % era.parameters.epochLength;
};

function epochLength(eraSummaries, slot) {
  const era = findEra(eraSummaries, slot);
  return era.parameters.epochLength;
};


function slotToEpoch(eraSummaries, slot) {
  const era = findEra(eraSummaries, slot);
  const deltaSlot = slot - era.start.slot;
  const epoch = Math.floor(deltaSlot / era.parameters.epochLength);
  return era.start.epoch + epoch;
};

function fetchPots(ws) {
  return ws.queryLedgerState("treasuryAndReserves");
}

function fetchPools(ws) {
  return ws.queryLedgerState("stakePools", { includeStake: true });
}

function fetchNonces(ws) {
  return ws.queryLedgerState("nonces");
}
