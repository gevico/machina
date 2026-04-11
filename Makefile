SHELL := /bin/bash

.PHONY: all build release test test-backend test-frontend test-integration \
        clippy fmt fmt-check docs clean

all: build

build:
	cargo build --workspace

release:
	cargo build --workspace --release

test:
	cargo test --workspace

test-backend:
	cargo test -p machina-tests backend

test-frontend:
	cargo test -p machina-tests frontend

test-integration:
	cargo test -p machina-tests integration exec

clippy:
	cargo clippy --workspace -- -D warnings -A clippy::pedantic -A clippy::nursery

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all --check

docs:
	cargo doc --workspace

clean:
	cargo clean
