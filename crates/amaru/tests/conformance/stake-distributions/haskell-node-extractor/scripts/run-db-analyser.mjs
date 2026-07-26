#!/usr/bin/env node

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

import * as cp from "node:child_process";
import * as fs from "node:fs";
import * as path from "node:path";
import { ogmios } from "@cardano-ogmios/mdk";

const packageJsonPath = new URL(path.join(path.dirname(import.meta.url), "..", "package.json"));
const ogmiosUrl = process.env.OGMIOS_URL ?? "ws://127.0.0.1:1337";

const network = (process.argv[2] ?? process.env.CARDANO_NODE_NETWORK).toLowerCase();

if (!["preview", "preprod", "mainnet"].includes(network)) {
  console.log(`Missing or invalid network.
Usage:
    ./run-db-analyser.mjs NETWORK

Arguments:
    NETWORK:  One of 'preview', 'preprod' or 'mainnet'

The starting point is read from package.json at:
    amaru.stakeDistributions.startFrom.<NETWORK>
`);
  process.exit(1);
}

await run(analyseContinuously);

async function run(job) {
  try {
    await job(readPackageJson().amaru.stakeDistributions.startFrom[network]);
  } catch (err) {
    console.log(err);
    await sleep(1000);
    await run(job);
  }
}

function updatePackageJson(network, point) {
  const packageJson = readPackageJson();
  packageJson.amaru.stakeDistributions.startFrom[network] = point;
  fs.writeFileSync(packageJsonPath, `${JSON.stringify(packageJson, null, 2)}\n`);
}

function readPackageJson() {
  return JSON.parse(fs.readFileSync(packageJsonPath, "utf8"));
}

async function analyseContinuously(startPoint) {
  await ogmios(async (ws, done) => {
    const eraSummaries = await ws.queryLedgerState("eraSummaries");

    const chainFollower = await ws.newChainFollower([startPoint]);

    let previousEpoch = null;
    let previousSlot = startPoint.slot;
    let previousHash = startPoint.id;
    let analyzeFrom = startPoint.slot;

    for await (const { block } of chainFollower()) {
      const currentEpoch = slotToEpoch(eraSummaries, block.slot)

      if (previousEpoch === null) {
        previousEpoch = currentEpoch;
      }

      if (currentEpoch > previousEpoch) {
        await storeLedgerAt(previousEpoch, analyzeFrom, previousSlot);
        analyzeFrom = previousSlot;
        updatePackageJson(network, { slot: previousSlot, id: previousHash });
      }

      previousEpoch = currentEpoch;
      previousSlot = block.slot;
      previousHash = block.id;
    }

    done();
  });
}

function storeLedgerAt(epoch, analyzeFrom, at) {
  const db = process.env.CARDANO_NODE_DB;
  const config =  process.env.CARDANO_NODE_CONFIG;
  const args = [ "--in-mem", "--db", db, "--config", config, "--analyse-from", String(analyzeFrom), "--store-ledger", String(at)];
  const cmd = `db-analyser`;
  console.log(`epoch=${epoch}: ${cmd} ${args.slice(5).join(" ")}`);
  cp.execSync(`${cmd} ${args.join(" ")}`, { stdio: 'pipe' });
}

async function sleep(ms) {
  await new Promise(resolve => setTimeout(resolve, ms));
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
