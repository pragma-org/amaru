all: setup build run

setup:
	rustup target add wasm32-unknown-unknown

build:
	RUSTFLAGS='--cfg getrandom_backend="wasm_js"' cargo build --artifact-dir assets

run:
	node index.mjs
