# AirCard -- Tauri + Rust.
#
# `legacy/` still holds the previous SwiftUI + Python implementation, and its test
# suite is the behavioural spec the Rust port is checked against:
#   make test-legacy
#
# `make test` is the fast suite: the two library crates, no Tauri and no window.

.PHONY: test test-all test-legacy test-device check front install dev bundle sign clean

test: ## library tests only (fast, no window)
	cargo test -p aircard-core -p aircard-device -p aircard-apple-ffi

test-all: ## every Rust test, including the Tauri shell
	cargo test --workspace

test-legacy: ## the ported Python suite, still the spec
	cd legacy && python3 -m unittest discover -s tests

test-device: ## hardware tests, against a plugged-in iPhone; one session at a time
	cargo test -p aircard-apple-ffi -- --ignored --nocapture --test-threads=1

check: ## types and the whole workspace, without running tests
	npm run check
	cargo check --workspace

front: ## frontend production build
	npm run build

install: ## frontend dependencies
	npm install

dev: ## run the app with hot reload
	npm run tauri dev

bundle: ## macOS .app and .dmg, then signed and verified
	npm run tauri build
	./scripts/sign-macos.sh

sign: ## re-sign and verify an existing bundle
	./scripts/sign-macos.sh

clean:
	cargo clean
	rm -rf dist node_modules
