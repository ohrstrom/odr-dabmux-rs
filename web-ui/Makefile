SHELL := bash
.ONESHELL:
.SHELLFLAGS := -eu -o pipefail -c
.DELETE_ON_ERROR:
MAKEFLAGS += --warn-undefined-variables
MAKEFLAGS += --no-builtin-rules


.PHONY: fmt
fmt:
	cargo fmt --all

.PHONY: dev
dev:
	bun --hot ./dev.ts

.PHONY: build
build:
	bun ./build.ts
