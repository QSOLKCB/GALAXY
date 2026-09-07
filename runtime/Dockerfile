# SPDX-License-Identifier: Apache-2.0
FROM rust:1.85.1-bookworm AS build
WORKDIR /opt/galaxy
COPY rust-toolchain.toml ./
COPY rust/ rust/
COPY runtime/ runtime/
COPY data/uff/ data/uff/
COPY tests/uff-reference.json tests/uff-reference.json
RUN cargo build --manifest-path runtime/Cargo.toml --release --locked --bins
RUN mkdir -p /opt/dependency-licenses && find /usr/local/cargo/registry/src -maxdepth 3 -type f \
    \( -iname 'LICENSE*' -o -iname 'COPYING*' -o -iname 'NOTICE*' \) \
    -exec cp --parents '{}' /opt/dependency-licenses/ \;

FROM ubuntu:24.04
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
    ca-certificates libvulkan1 openssh-server && rm -rf /var/lib/apt/lists/*
ENV NVIDIA_VISIBLE_DEVICES=all
ENV NVIDIA_DRIVER_CAPABILITIES=graphics,utility,compute
COPY --from=build /opt/galaxy/runtime/target/release/galaxy-runtime /usr/local/bin/galaxy-runtime
COPY --from=build /opt/galaxy/runtime/target/release/galaxy-u64 /usr/local/bin/galaxy-u64
COPY runtime/jobs/ /opt/galaxy/jobs/
COPY --from=build /opt/dependency-licenses/ /usr/share/doc/galaxy-runtime/dependencies/
COPY LICENSE NOTICE.md /usr/share/doc/galaxy-runtime/
COPY data/uff/NOTICE /usr/share/doc/galaxy-runtime/UFF-NOTICE
WORKDIR /workspace
ENTRYPOINT ["galaxy-runtime"]
CMD ["devices"]
