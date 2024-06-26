ARG BUILDER_IMAGE
FROM ${BUILDER_IMAGE} as build

ARG UNAME_ARCH
USER root

# Required to build in --offline mode
ENV CARGO_HOME=/src/.cargo

# Add brupop source
ADD ./ /src/

# Ensure cargo dependencies are fetched and available in the Docker context
RUN cargo fetch --locked --manifest-path /src/Cargo.toml

# Builds brupop binaries
RUN cargo install --offline --locked --target ${UNAME_ARCH}-bottlerocket-linux-musl --path /src/agent --root /src/agent && \
    cargo install --offline --locked --target ${UNAME_ARCH}-bottlerocket-linux-musl --path /src/apiserver --root /src/apiserver && \
    cargo install --offline --locked --target ${UNAME_ARCH}-bottlerocket-linux-musl --path /src/controller --root /src/controller

# Gather licenses of dependencies
RUN /usr/libexec/tools/bottlerocket-license-scan \
    --clarify /src/clarify.toml \
    --spdx-data /usr/libexec/tools/spdx-data \
    --out-dir /licenses \
    cargo --offline --locked /src/Cargo.toml


FROM scratch

ARG UNAME_ARCH

# Copy CA certificates store
COPY --from=build /etc/ssl /etc/ssl
COPY --from=build /etc/pki /etc/pki

# Copy rust binaries into resulting image
COPY --from=build /src/apiserver/bin/apiserver ./
COPY --from=build /src/agent/bin/agent ./
COPY --from=build /src/controller/bin/controller ./

# Copy license data
COPY --from=build /src/COPYRIGHT /src/LICENSE-MIT /src/LICENSE-APACHE /licenses/bottlerocket-update-operator/
# Direct rust dependencies of the update-operator
COPY --from=build /licenses /licenses
# Build dependencies from the Bottlerocket SDK
COPY --from=build \
    /usr/share/licenses/bottlerocket-sdk-musl-${UNAME_ARCH} \
    /usr/share/licenses/rust \
    /licenses/bottlerocket-sdk/
