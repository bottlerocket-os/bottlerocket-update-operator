ARG BUILDER_IMAGE
FROM ${BUILDER_IMAGE} as build

ARG UNAME_ARCH
USER root

# We need these environment variables set for building the `openssl-sys` crate
ENV PKG_CONFIG_PATH=/${UNAME_ARCH}-bottlerocket-linux-musl/sys-root/usr/lib/pkgconfig
ENV PKG_CONFIG_ALLOW_CROSS=1
ENV CARGO_HOME=/src/.cargo
ENV OPENSSL_STATIC=true

ADD ./ /src/
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
    /usr/share/licenses/bottlerocket-sdk-musl \
    /usr/share/licenses/rust \
    /usr/share/licenses/openssl \
    /licenses/bottlerocket-sdk/

# Expose apiserver port
EXPOSE 8443
