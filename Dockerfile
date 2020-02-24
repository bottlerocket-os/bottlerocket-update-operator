# syntax=docker/dockerfile:experimental
FROM golang:1.13 as build
ARG GO_LDFLAGS=
ARG GOARCH=
ARG SHORT_SHA=
ENV GOPROXY=direct
COPY go.mod go.sum /go/src/github.com/bottlerocket-os/bottlerocket-update-operator/
WORKDIR /go/src/github.com/bottlerocket-os/bottlerocket-update-operator
RUN go mod download
COPY . /go/src/github.com/bottlerocket-os/bottlerocket-update-operator/
RUN make -e build GOBIN=/ CGO_ENABLED=0

# Build minimal container with a static build of dogswatch.
FROM scratch as update-operator
COPY --from=build /bottlerocket-update-operator /etc/ssl /
ENTRYPOINT ["/bottlerocket-update-operator"]
CMD ["-help"]

FROM build as test
# Accept a cache-busting value to explicitly run tests.
ARG NOCACHE=
RUN make -e test

# Make container the output of a plain 'docker build'.
FROM update-operator
