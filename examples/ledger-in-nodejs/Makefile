all: setup build run

setup:
	rustup target add wasm32-unknown-unknown

build:
	cargo build --artifact-dir assets

run:
	node index.mjs