# Makefile - Bottlerocket update operator build and development targets
#
# ecr_uri=$(aws ecr describe-repositories --repository bottlerocket-os/bottlerocket-update-operator --query 'repositories[].repositoryUri' --output text)
#
# make container IMAGE_REPO_NAME="$ecr_uri"

# SHELL is set as bash to use some bashisms.
SHELL = bash
# IMAGE_NAME is the full name of the container image being built. This may be
# specified to fully control the name of the container image's tag.
IMAGE_NAME = $(IMAGE_REPO_NAME)$(IMAGE_ARCH_SUFFIX):$(IMAGE_VERSION)$(addprefix -,$(SHORT_SHA))
# IMAGE_REPO_NAME is the image's full name in a container image registry. This
# could be an ECR Repository name or a Docker Hub name such as
# `example-org/example-image`. If the repository includes the architecture name,
# IMAGE_ARCH_SUFFIX must be overridden as needed.
IMAGE_REPO_NAME = $(notdir $(shell pwd -P))
# IMAGE_VERSION is the semver version that's tagged on the image.
IMAGE_VERSION = $(shell cat VERSION)
# SHORT_SHA is the revision that the container image was built with.
SHORT_SHA = $(shell git describe --abbrev=8 --always --dirty='-dev' --exclude '*' 2>/dev/null || echo "unknown")
# IMAGE_ARCH_SUFFIX is the runtime architecture designator for the container
# image, it is appended to the IMAGE_NAME unless the name is specified.
IMAGE_ARCH_SUFFIX = $(addprefix -,$(ARCH))
# BUILDSYS_SDK_IMAGE is the Bottlerocket SDK image used for license scanning.
BUILDSYS_SDK_IMAGE ?= bottlerocket/sdk-x86_64:v0.8
# LICENSES_IMAGE_NAME is the name of the container image that has LICENSE files
# for distribution.
LICENSES_IMAGE = $(IMAGE_NAME)-licenses
# DESTDIR is where the release artifacts will be written.
DESTDIR = .
# DISTFILE is the path to the dist target's output file - the container image
# tarball.
DISTFILE = $(DESTDIR:/=)/$(subst /,_,$(IMAGE_NAME)).tar.gz

# These values derive ARCH and DOCKER_ARCH which are needed by dependencies in
# image build defaulting to system's architecture when not specified.
#
# UNAME_ARCH is the runtime architecture of the building host.
UNAME_ARCH = $(shell uname -m)
# ARCH is the target architecture which is being built for.
ARCH = $(lastword $(subst :, ,$(filter $(UNAME_ARCH):%,x86_64:amd64 aarch64:arm64)))
# DOCKER_ARCH is the docker specific architecture specifier used for building on
# multiarch container images.
DOCKER_ARCH = $(lastword $(subst :, ,$(filter $(ARCH):%,amd64:amd64 arm64:arm64v8)))

.PHONY: all build check

# Go compliation specific to selected build and target system architecture.
GOPKG = github.com/bottlerocket-os/bottlerocket-update-operator
GOPKGS = $(GOPKG) $(GOPKG)/pkg/...
export GOBIN = $(shell pwd -P)/bin
export GOARCH = $(ARCH)

.DEFAULT_GOAL = build

# Run all build tasks for this daemon & its container image.
all: build test container check

# Build the daemon and tools into GOBIN
build:
	go install -v $(GOPKG)

# Run Go tests for daemon and tools.
#
# Tests run only with the native GOARCH of the system.
test: GOARCH=
# Use debuggable build to capture more logging for diagnosing failing tests.
test: GO_LDFLAGS +=-X $(GOPKG)/pkg/logging.DebugEnable=true
test:
	go test -race -ldflags '$(GO_LDFLAGS)' $(GOPKGS)

# Build a container image for daemon and tools.
container: licenses
	docker build \
		--network=host \
		--build-arg 'GO_LDFLAGS=$(GO_LDFLAGS)' \
		--build-arg 'GOARCH=$(GOARCH)' \
		--build-arg 'SHORT_SHA=$(SHORT_SHA)' \
		--build-arg 'LICENSES_IMAGE=$(LICENSES_IMAGE)' \
		--build-arg 'GOLANG_IMAGE=$(BUILDSYS_SDK_IMAGE)' \
		--target='update-operator' \
		--tag '$(IMAGE_NAME)' \
		.

# Build and test in a container.
container-test: sdk-image licenses
	docker build \
		--network=host \
		--build-arg 'GO_LDFLAGS=$(GO_LDFLAGS)' \
		--build-arg 'GOARCH=$(GOARCH)' \
		--build-arg 'SHORT_SHA=$(SHORT_SHA)' \
		--build-arg 'NOCACHE=$(shell date +'%s')' \
		--build-arg 'LICENSES_IMAGE=$(LICENSES_IMAGE)' \
		--build-arg 'GOLANG_IMAGE=$(BUILDSYS_SDK_IMAGE)' \
		--target='test' \
		--tag '$(IMAGE_NAME)-test' \
		.

# Build container image with debug-configured daemon.
debug: GO_LDFLAGS +=-X $(GOPKG)/pkg/logging.DebugEnable=true
debug: IMAGE_NAME := $(IMAGE_NAME)-debug
debug: container

# Create a distribution container image tarball for release.
dist: container check
	@mkdir -p $(dir $(DISTFILE))
	docker save $(IMAGE_NAME) | gzip > '$(DISTFILE)'

# Run checks on the container image.
check: check-executable check-licenses

# Check that the container's executable works.
check-executable:
	@echo "Running check: $@"
	@echo "Checking if the container image's ENTRYPOINT is executable..."
	docker run --rm $(IMAGE_NAME) -help 2>&1 \
		| grep -q '/bottlerocket-update-operator'

# Check that the container has LICENSE files included for its dependencies.
check-licenses: CONTAINER_HASH=$(shell echo "$(LICENSES_IMAGE_NAME)$$(pwd -P)" | sha256sum - | awk '{print $$1}' | head -c 16)
check-licenses: CHECK_CONTAINER_NAME=check-licenses-$(CONTAINER_HASH)
check-licenses:
	@echo "Running check: $@"
	@-if docker inspect $(CHECK_CONTAINER_NAME) &>/dev/null; then\
		docker rm $(CHECK_CONTAINER_NAME) &>/dev/null; \
	fi
	@docker create --name $(CHECK_CONTAINER_NAME) $(IMAGE_NAME) >/dev/null
	@echo "Checking if container image included dependencies' LICENSE files..."
	@docker export $(CHECK_CONTAINER_NAME) | tar -tf - \
		| grep usr/share/licenses/bottlerocket-update-operator/vendor \
		| grep -q LICENSE || { \
			echo "Container image is missing required LICENSE files (checked $(IMAGE_NAME))"; \
			docker rm $(CHECK_CONTAINER_NAME) &>/dev/null; \
			exit 1; \
		}
	@-docker rm $(CHECK_CONTAINER_NAME) &>/dev/null

# Clean the build artifacts on disk.
clean:
	rm -f -- $(foreach binpkg,$(GOPKG) $(wildcard ./cmd/*),'$(GOBIN)/$(notdir $(binpkg))')
	rm -f -- '$(DISTFILE)'

# Development targets

# Deploy the development stack to the environment selected cluster. This
# requires that the image be present as the default name OR specified & pushed
# to a cluster-reachable location.
deploy-dev:
	sed 's,@containerRef@,$(IMAGE_NAME),g' ./dev/deployment.yaml \
		| kubectl apply -f -

# Rollout updates resources and bounces them to effectively "restart" the
# collective service.
rollout: deploy-dev
	kubectl -n bottlerocket rollout restart deployment/update-operator-controller
	kubectl -n bottlerocket rollout restart daemonset/update-operator-agent

# Load the docker image into cluster's container image storage ("kind"
# development cluster specific).
kind-load: container
	kind load docker-image $(IMAGE_NAME)

# Rollout but for "kind" development clusters (see `rollout').
kind-rollout: kind-load rollout

# Cluster creates a "kind" based cluster for development.
kind-cluster:
	@hash kind
	kind create cluster --config ./dev/cluster.yaml

dashboard:
	@echo "Dashboard deployment, as configured here, is unauthenticated & intended"
	@echo "for development clusters that are private or only locally accessible."
	@echo
	@echo "To configure, run, and proxy kubernetes-dashboard, use:"
	@echo
	@echo "  \$$ make unsafe-dashboard"
	@echo
	@exit 1

# Spin up a service for the Kubernetes Dashboard - this can be unsafe as any
# access to ClusterIP bound services will automatically authenticate clients as
# fully-privileged.
unsafe-dashboard:
	kubectl apply -f ./dev/dashboard.yaml
	@echo 'Visit dashboard at: http://localhost:8001/api/v1/namespaces/kube-system/services/https:kubernetes-dashboard:/proxy/'
	kubectl proxy

# Print the Node operational management status.
get-nodes-status:
	kubectl get nodes -o json | jq -C -S '.items | map(.metadata|{(.name): (.annotations|to_entries|map(select(.key|startswith("bottlerocket")))|from_entries)}) | add'

# sdk-image fetches and loads the bottlerocket SDK container image for build and
# license collection.
sdk-image: BUILDSYS_SDK_IMAGE_URL=https://cache.bottlerocket.aws/$(BUILDSYS_SDK_IMAGE).tar.gz
sdk-image:
	docker inspect $(BUILDSYS_SDK_IMAGE) 2>&1 >/dev/null \
		|| curl -# -qL $(BUILDSYS_SDK_IMAGE_URL) | docker load -i /dev/stdin

# licenses builds a container image with the LICENSE & legal files from the
# source's dependencies. This image is consumed during build (see `container`)
# to COPY the result into the distributed container image.
#
# Dependencies are walked using the Go toolchain and then processed with the
# project's license scanner, which is built into the bottlerocket SDK image.
licenses: sdk-image go.mod go.sum
	docker build \
		--network=host \
		--build-arg SDK_IMAGE=$(BUILDSYS_SDK_IMAGE) \
		--build-arg GOLANG_IMAGE=$(BUILDSYS_SDK_IMAGE) \
		-t $(LICENSES_IMAGE) \
		-f build/Dockerfile.licenses .
