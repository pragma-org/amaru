import fs from 'node:fs/promises';

const wasmBuffer = await fs.readFile('assets/amaru_ledger_in_nodejs.wasm');
const wasm = await WebAssembly.instantiate(wasmBuffer);

wasm.instance.exports.ledger();

console.log("Done");