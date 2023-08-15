TOP := $(dir $(abspath $(firstword $(MAKEFILE_LIST))))

.PHONY: image fetch check-licenses build brupop-image clean

# IMAGE_NAME is the full name of the container image being built. This may be
# specified to fully control the name of the container image's tag.
IMAGE_NAME ?= $(IMAGE_REPO_NAME)$(IMAGE_ARCH_SUFFIX):$(IMAGE_VERSION)$(addprefix -,$(SHORT_SHA))
# IMAGE_REPO_NAME is the image's full name in a container image registry. This
# could be an ECR Repository name or a Docker Hub name such as
# `example-org/example-image`. If the repository includes the architecture name,
# IMAGE_ARCH_SUFFIX must be overridden as needed.
IMAGE_REPO_NAME = $(shell basename `git rev-parse --show-toplevel`)
# IMAGE_VERSION is the semver version that's tagged on the image and helm charts.
IMAGE_VERSION = $(shell cat VERSION)
# SHORT_SHA is the revision that the container image was built with.
SHORT_SHA ?= $(shell git describe --abbrev=8 --always --dirty='-dev' --exclude '*' 2>/dev/null || echo "unknown")
# IMAGE_ARCH_SUFFIX is the runtime architecture designator for the container
# image, it is appended to the IMAGE_NAME unless the name is specified.
IMAGE_ARCH_SUFFIX ?= $(addprefix -,$(ARCH))

# UNAME_ARCH is the runtime architecture of the building host.
UNAME_ARCH = $(shell uname -m)
# ARCH is the target architecture which is being built for.
ARCH ?= $(lastword $(subst :, ,$(filter $(UNAME_ARCH):%,x86_64:amd64 aarch64:arm64)))

# DESTDIR is where the release artifacts will be written.
DESTDIR ?= .
# DISTFILE is the path to the dist target's output file - the container image
# tarball.
DISTFILE ?= $(DESTDIR:/=)/$(subst /,_,$(IMAGE_NAME)).tar.gz

BOTTLEROCKET_SDK_VERSION = v0.33.0
BOTTLEROCKET_SDK_ARCH = $(UNAME_ARCH)

# Tools used during the chart release lifecycle
export KUBECONFORM_VERSION = v0.6.3
export HELMV3_VERSION = v3.6.3

BUILDER_IMAGE = public.ecr.aws/bottlerocket/bottlerocket-sdk-$(BOTTLEROCKET_SDK_ARCH):$(BOTTLEROCKET_SDK_VERSION)

export CARGO_ENV_VARS = CARGO_REGISTRIES_CRATES_IO_PROTOCOL=sparse
export CARGO_HOME = $(TOP)/.cargo

export CHART_BUILD_DIR := $(TOP)/chartbuild
export CHART_TMP_DIR := $(CHART_BUILD_DIR)/tmp
export CHART_TOOLS_DIR := $(CHART_BUILD_DIR)/tools
export CHARTS_DIR := $(TOP)/deploy/charts
export PATH := $(CHART_TOOLS_DIR):$(PATH)

image: check-licenses brupop-image

# Fetches crates from upstream
fetch:
	$(CARGO_ENV_VARS) cargo fetch --locked

dev-tools:
	cargo install cargo-insta

# Checks allowed/denied upstream licenses against the deny.toml
check-licenses: fetch
	docker run --rm \
		--network none \
		--user "$(shell id -u):$(shell id -g)" \
		--security-opt label=disable \
		--env CARGO_HOME="/src/.cargo" \
		--volume "$(TOP):/src" \
		--workdir "/src/" \
		"$(BUILDER_IMAGE)" \
		bash -c "$(CARGO_ENV_VARS) cargo deny --all-features check --disable-fetch licenses bans sources"

# Builds, Lints, and Tests the Rust workspace locally
build: check-licenses
	$(CARGO_ENV_VARS) cargo fmt -- --check
	$(CARGO_ENV_VARS) cargo test --locked
	$(CARGO_ENV_VARS) cargo build --locked

# Builds only the brupop image. Useful target for CI/CD, releasing, etc.
brupop-image:
	docker build $(DOCKER_BUILD_FLAGS) \
		--build-arg UNAME_ARCH="$(UNAME_ARCH)" \
		--build-arg BUILDER_IMAGE="$(BUILDER_IMAGE)" \
		--tag "$(IMAGE_NAME)" \
		-f Dockerfile .

dist: check-licenses brupop-image
	@mkdir -p $(dir $(DISTFILE))
	docker save $(IMAGE_NAME) | gzip > '$(DISTFILE)'

clean:
	-rm -rf target
	-rm -rf chartbuild
	rm -f -- '$(DISTFILE)'

check-crd-golden-diff:
	# =^..^=   =^..^=   =^..^=   =^..^=   =^..^=   =^..^=   =^..^=   =^..^=   =^..^=
	# This useful make target visualizes the diff between the CRD template in
	# the helm chart compared to the generated golden file (with real values)
	# from the rust definitions. This is useful to ensure there are no hanging changes
	# that need to be made to the template.
	# You should expect to see a 1:1 relationship between a template and a value.
	# =^..^=   =^..^=   =^..^=   =^..^=   =^..^=   =^..^=   =^..^=   =^..^=   =^..^=
	diff --color \
		deploy/charts/bottlerocket-shadow/templates/custom-resource-definition.yaml \
		deploy/tests/golden/custom-resource-definition.yaml || return 0

manifest:
	echo --- > bottlerocket-update-operator.yaml && \
	kubectl create namespace brupop-bottlerocket-aws \
		--dry-run=client \
		-o yaml >> bottlerocket-update-operator.yaml && \
	helm template deploy/charts/bottlerocket-shadow >> bottlerocket-update-operator.yaml && \
	helm template deploy/charts/bottlerocket-update-operator >> bottlerocket-update-operator.yaml

verify-charts:
	scripts/validate-charts.sh
	scripts/validate-chart-versions.sh
	scripts/lint-charts.sh

package-charts:
	mkdir -p $(CHART_BUILD_DIR)
	scripts/package-charts.sh

publish-charts: package-charts
	scripts/publish-charts.sh

install-charts-toolchain:
	mkdir -p $(CHART_BUILD_DIR)
	mkdir -p $(CHART_TOOLS_DIR)
	scripts/install-toolchain.sh

version:
	@echo ${IMAGE_VERSION}
