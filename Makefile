# One entry point for every check. CI runs `make ci`; agents and humans run
# `make check`. Keeping the definition here (rather than duplicated in CI YAML)
# means a workflow that loops "run the checks, fix what failed" always drives
# the same commands CI will.

CARGO ?= cargo

.PHONY: all check ci fmt fmt-check lint test build release run doc clean

all: check

## Fast feedback loop: format, lint, and test.
check: fmt lint test

## Exactly what CI enforces; fails instead of rewriting files.
ci: fmt-check lint test build

## Rewrite sources to canonical formatting.
fmt:
	$(CARGO) fmt --all

## Verify formatting without modifying files.
fmt-check:
	$(CARGO) fmt --all -- --check

## Clippy across every target, warnings treated as errors.
lint:
	$(CARGO) clippy --all-targets --all-features -- -D warnings

## Unit tests, integration tests, and doctests.
test:
	$(CARGO) test --all-features

build:
	$(CARGO) build --all-targets

release:
	$(CARGO) build --release

run:
	$(CARGO) run -- run --name dev --workers 2

doc:
	$(CARGO) doc --no-deps --document-private-items

clean:
	$(CARGO) clean
