SHELL := bash
.ONESHELL:
.SHELLFLAGS := -eu -o pipefail -c
.DELETE_ON_ERROR:
MAKEFLAGS += --warn-undefined-variables
MAKEFLAGS += --no-builtin-rules


.PHONY: fmt
fmt:
	cargo fmt --all

.PHONY: clippy
clippy:
	cargo clippy --no-deps --all-targets -- -D warnings

.PHONY: clean
clean:
	cargo clean

.PHONY: build
build:
	cargo build

.PHONY: build-release
build-release:
	cargo build --release

.PHONY: release
release:
	cargo release rc --execute

.PHONY: release-rc
release-rc:
	cargo release rc --execute
	# notice output: git push origin main --follow-tags
	echo "git push origin main --follow-tags"

.PHONY: install
install:
	cargo install --path .

# dev helpers
.PHONY: llm-dump
llm-dump:
	@find src -type f -name "*.rs" | sort | while read -r file; do \
	  echo "# $$file"; \
	  echo; \
	  cat "$$file"; \
	  echo; \
	  echo; \
	done > docs.local/llm-dump.txt
