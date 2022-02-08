TOP := $(dir $(abspath $(firstword $(MAKEFILE_LIST))))

.PHONY: image fetch check-licenses build brupop-image clean

# IMAGE_NAME is the full name of the container image being built. This may be
# specified to fully control the name of the container image's tag.
IMAGE_NAME = $(IMAGE_REPO_NAME)$(IMAGE_ARCH_SUFFIX):$(IMAGE_VERSION)$(addprefix -,$(SHORT_SHA))
# IMAGE_REPO_NAME is the image's full name in a container image registry. This
# could be an ECR Repository name or a Docker Hub name such as
# `example-org/example-image`. If the repository includes the architecture name,
# IMAGE_ARCH_SUFFIX must be overridden as needed.
IMAGE_REPO_NAME = $(shell basename `git rev-parse --show-toplevel`)
# IMAGE_VERSION is the semver version that's tagged on the image.
IMAGE_VERSION = $(shell cat VERSION)
# SHORT_SHA is the revision that the container image was built with.
SHORT_SHA = $(shell git describe --abbrev=8 --always --dirty='-dev' --exclude '*' 2>/dev/null || echo "unknown")
# IMAGE_ARCH_SUFFIX is the runtime architecture designator for the container
# image, it is appended to the IMAGE_NAME unless the name is specified.
IMAGE_ARCH_SUFFIX = $(addprefix -,$(ARCH))

# UNAME_ARCH is the runtime architecture of the building host.
UNAME_ARCH = $(shell uname -m)
# ARCH is the target architecture which is being built for.
ARCH = $(lastword $(subst :, ,$(filter $(UNAME_ARCH):%,x86_64:amd64 aarch64:arm64)))

BOTTLEROCKET_SDK_VERSION = v0.25.1
BOTTLEROCKET_SDK_ARCH = $(UNAME_ARCH)

BUILDER_IMAGE = public.ecr.aws/bottlerocket/bottlerocket-sdk-$(BOTTLEROCKET_SDK_ARCH):$(BOTTLEROCKET_SDK_VERSION)

export CARGO_HOME = $(TOP)/.cargo

image: build brupop-image

# Fetches crates from upstream
fetch:
	cargo fetch --locked

check-licenses: fetch
	docker run --rm \
		--network none \
		--user "$(shell id -u):$(shell id -g)" \
		--security-opt label=disable \
		--env CARGO_HOME="/src/.cargo" \
		--volume "$(TOP):/src" \
		--workdir "/src/" \
		"$(BUILDER_IMAGE)" \
		bash -c "cargo deny --all-features check --disable-fetch licenses bans sources"

# Builds, Lints and Tests the Rust workspace
build: fetch check-licenses
	cargo fmt -- --check
	cargo build --locked
	cargo test --locked

brupop-image: fetch check-licenses
	docker build $(DOCKER_BUILD_FLAGS) \
		--build-arg UNAME_ARCH="$(UNAME_ARCH)" \
		--build-arg BUILDER_IMAGE="$(BUILDER_IMAGE)" \
		--tag "$(IMAGE_NAME)" \
		--network none \
		-f Dockerfile .

clean:
	-rm -rf target
