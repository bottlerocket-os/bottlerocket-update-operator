TOP := $(dir $(abspath $(firstword $(MAKEFILE_LIST))))

.PHONY: image fetch build brupop-image

UNAME_ARCH = $(shell uname -m)
ARCH ?= $(lastword $(subst :, ,$(filter $(UNAME_ARCH):%,x86_64:amd64 aarch64:arm64)))

BOTTLEROCKET_SDK_VERSION = v0.23.0
BOTTLEROCKET_SDK_ARCH = $(UNAME_ARCH)

BUILDER_IMAGE = public.ecr.aws/bottlerocket/bottlerocket-sdk-$(BOTTLEROCKET_SDK_ARCH):$(BOTTLEROCKET_SDK_VERSION)

export CARGO_HOME = $(TOP)/.cargo

image: build brupop-image

# Fetches crates from upstream
fetch:
	cargo fetch --locked

# Builds, Lints and Tests the Rust workspace
build: fetch
	cargo fmt -- --check
	cargo build --locked
	cargo test --locked

brupop-image: fetch
	docker build $(DOCKER_BUILD_FLAGS) \
		--build-arg UNAME_ARCH="$(UNAME_ARCH)" \
		--build-arg BUILDER_IMAGE="$(BUILDER_IMAGE)" \
		--tag "brupop-$(ARCH)" \
		--network none \
		-f Dockerfile .
