# syntax=docker/dockerfile:experimental

# LICENSES_IMAGE is a container image that contains license files for the source
# and its dependencies. When building with `make container`, the licenses
# container image is built and provided as LICENSE_IMAGE.
ARG LICENSES_IMAGE=scratch

# GOLANG_IMAGE is the go toolchain container image used to build the update
# operator.
ARG GOLANG_IMAGE=golang:1.16.5

FROM $LICENSES_IMAGE as licenses
# Set WORKDIR to create /licenses/ if the directory is missing.
#
# Having an image with /licenses/ lets scratch be substituted in when
# LICENSES_IMAGE isn't provided. For example, a user can manually run `docker
# build -t neio:latest .` to build a working image without providing an expected
# LICENSES_IMAGE.
WORKDIR /licenses/

FROM $GOLANG_IMAGE as build
USER root
ENV GOPROXY=direct
WORKDIR /src
COPY go.mod go.sum /src/
RUN go mod download
ARG GO_LDFLAGS=
ARG GOARCH=
ARG SHORT_SHA=
COPY ./ /src/
RUN make -e build GOBIN=/ CGO_ENABLED=0

# This stage provides certificates (to be copied) from Amazon Linux 2.
FROM amazonlinux:2 as al2

# Build minimal container with a static build of the update operator executable.
FROM scratch as update-operator
COPY --from=al2 /etc/ssl /etc/ssl
COPY --from=al2 /etc/pki /etc/pki
COPY --from=build /src/COPYRIGHT /src/LICENSE-* /usr/share/licenses/bottlerocket-update-operator/
COPY --from=licenses /licenses/ /usr/share/licenses/bottlerocket-update-operator/vendor/
COPY --from=build /bottlerocket-update-operator /
ENTRYPOINT ["/bottlerocket-update-operator"]
CMD ["-help"]

FROM build as test
# Accept a cache-busting value to explicitly run tests.
ARG NOCACHE=
RUN make -e test

# Make container the output of a plain 'docker build'.
FROM update-operator
