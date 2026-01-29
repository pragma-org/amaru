#!/usr/bin/env node
'use strict';

const blake2b = require('blake2b');
const cp = require('node:child_process');
const fs = require('node:fs');
const path = require('node:path');

function usage(exitCode = 1, msg) {
  const script = path.basename(process.argv[1]);

  const lines = [
    msg ? `Error: ${msg}\n` : '',
    `Usage: node ${script} <cborHex> <description> [wellFormed]`,
    ``,
    `  <cborHex>       CBOR payload as hex (no 0x prefix).`,
    `  <description>   Any non-empty string (quote it if it contains spaces).`,
    `  [wellFormed]    Optional: "true" or "false".`,
    `                  If omitted, it is considered ill-formed (wellFormed=false).`,
    ``,
    `Example:`,
    `  ./${script} a0 "minimal sample" true`,
  ].filter(Boolean);

  console.error(lines.join('\n'));

  process.exit(exitCode);
}

// Parsing inputs
let body = process.argv[2];
const description = process.argv[3];
const isValid = process.argv[4] == "true";

if (body === undefined || description === undefined) {
  usage(1, 'missing required arguments.');
}

try {
  body = Buffer.from(body, "hex");

  // Computing id
  const id = blake2b(32).update(body).digest("hex");

  // Creating new vector
  cp.execSync(`mkdir -p ${id}`);
  fs.writeFileSync(path.join(id, "sample.cbor"), body);
  fs.writeFileSync(path.join(id, "meta.json"), JSON.stringify({well_formed: isValid, description }));

  console.log(`generated new test under ./${id}`);
} catch (e) {
  usage(1, String(e));
}
