ARG ARCH
FROM public.ecr.aws/bottlerocket/bottlerocket-sdk-${ARCH}:v0.22.0 as build
ARG ARCH
ARG OPENSSL_VERSION=1.1.1k
ARG OPENSSL_SHA256SUM=892a0875b9872acd04a9fde79b1f943075d5ea162415de3047c327df33fbaee5
USER root

# Build openssl using musl toolchain for openssl-sys crate
RUN mkdir /musl && \
    echo "/musl/lib" >> /etc/ld-musl-${ARCH}.path && \
    ln -s /usr/include/${ARCH}-linux-gnu/asm /${ARCH}-bottlerocket-linux-musl/sys-root/usr/include/asm && \
    ln -s /usr/include/asm-generic /${ARCH}-bottlerocket-linux-musl/sys-root/usr/include/asm-generic && \
    ln -s /usr/include/linux /${ARCH}-bottlerocket-linux-musl/sys-root/usr/include/linux

RUN curl -O -sSL https://www.openssl.org/source/openssl-${OPENSSL_VERSION}.tar.gz && \
    echo "${OPENSSL_SHA256SUM} openssl-${OPENSSL_VERSION}.tar.gz" | sha256sum --check && \
    tar -xzf openssl-${OPENSSL_VERSION}.tar.gz && \
    cd openssl-${OPENSSL_VERSION} && \
    if [ ${ARCH} = "aarch64" ]; then CONFIGURE_ARGS="-mno-outline-atomics"; else CONFIGURE_ARGS=""; fi && \
    ./Configure no-shared no-async ${CONFIGURE_ARGS} -fPIC --prefix=/musl --openssldir=/musl/ssl linux-${ARCH} && \
    env C_INCLUDE_PATH=/musl/include/ make depend 2> /dev/null && \
    make -j && \
    make install_sw && \
    cd .. && rm -rf openssl-${OPENSSL_VERSION}

# We need these environment variables set for building the `openssl-sys` crate
ENV PKG_CONFIG_ALLOW_CROSS=1
ENV OPENSSL_STATIC=true
ENV OPENSSL_DIR=/musl

FROM build as brupopbuild
ARG ARCH
USER root

ADD ./ /src/
RUN cargo install --locked --target ${ARCH}-bottlerocket-linux-musl --path /src/agent --root /src/agent && \
    cargo install --locked --target ${ARCH}-bottlerocket-linux-musl --path /src/apiserver --root /src/apiserver && \
    cargo install --locked --target ${ARCH}-bottlerocket-linux-musl --path /src/controller --root /src/controller


FROM scratch
# Copy CA certificates store
COPY --from=build /etc/ssl /etc/ssl
COPY --from=build /etc/pki /etc/pki

# Copy rust binaries into resulting image
COPY --from=brupopbuild /src/apiserver/bin/apiserver ./
COPY --from=brupopbuild /src/agent/bin/agent ./
COPY --from=brupopbuild /src/controller/bin/controller ./

# Expose apiserver port
EXPOSE 8080
