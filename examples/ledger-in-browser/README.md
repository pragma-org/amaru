A simple example of `amaru-ledger` crate compiled into WASM and loaded via nodejs.

```console
cargo build --release --artifact-dir assets
wasm-strip assets/amaru_ledger_in_browser.wasm 
node index.mjs
```