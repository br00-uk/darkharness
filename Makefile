# One entry point for every check. CI runs the same commands, so a green
# `make ci` locally means a green CI run.

CARGO ?= cargo

.PHONY: all check ci fmt fmt-check lint test doc deps deny build release clean

all: check

## Fast feedback loop.
check: fmt lint test deps

## What CI enforces. Verifies formatting instead of rewriting it.
ci: fmt-check lint test deps deny build

fmt:
	$(CARGO) fmt --all

fmt-check:
	$(CARGO) fmt --all -- --check

lint:
	$(CARGO) clippy --workspace --all-targets --all-features -- -D warnings

## nextest does not run doctests, so run both.
test:
	$(CARGO) nextest run --workspace
	$(CARGO) test --workspace --doc

## Rules 12 to 17.
deps:
	$(CARGO) xtask check-deps

## Advisories, licences, bans, and sources.
deny:
	$(CARGO) deny check

build:
	$(CARGO) build --workspace --all-targets

release:
	$(CARGO) build --release

doc:
	$(CARGO) doc --workspace --no-deps

clean:
	$(CARGO) clean
