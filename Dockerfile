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

FROM scratch
# Copy CA certificates store
COPY --from=build /etc/ssl /etc/ssl
COPY --from=build /etc/pki /etc/pki

# Copy rust binaries into resulting image
COPY --from=build /src/apiserver/bin/apiserver ./
COPY --from=build /src/agent/bin/agent ./
COPY --from=build /src/controller/bin/controller ./

# Expose apiserver port
EXPOSE 8080
