#!/usr/bin/env node

const API_KEY = process.env.BLOCKFROST_API_KEY;

const epoch = process.argv[2];
const network = process.argv[3] || "mainnet";
const count = Number.parseInt(process.argv[4] || "1", 10);

if (!API_KEY || !epoch || !Number.isInteger(count) || count < 1) {
  process.exit(1);
}

const NETWORKS = {
  mainnet: "https://cardano-mainnet.blockfrost.io/api/v0",
  preprod: "https://cardano-preprod.blockfrost.io/api/v0",
  preview: "https://cardano-preview.blockfrost.io/api/v0",
};

const baseUrl = NETWORKS[network];
if (!baseUrl) process.exit(1);

async function fetchJSON(url) {
  const res = await fetch(url, {
    headers: { project_id: API_KEY },
  });

  if (!res.ok) {
    const details = await res.text().catch(() => "");
    const suffix = details ? `: ${details}` : "";
    console.error(`Blockfrost request failed (${res.status}) for ${url}${suffix}`);
    process.exit(1);
  }

  return res.json();
}

(async () => {
  const hashes = await fetchJSON(
    `${baseUrl}/epochs/${epoch}/blocks?order=desc&count=${count}`
  );

  const blocks = await Promise.all(
    hashes.map((h) => fetchJSON(`${baseUrl}/blocks/${h}`))
  );

  for (const b of blocks) {
    console.log(`${b.slot}.${b.hash}`);
  }
})();