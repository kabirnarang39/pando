# syntax=docker/dockerfile:1
# Multi-stage build: compile with the full toolchain, ship a ~30 MB distroless image
# running as a non-root user.

FROM rust:1-bookworm AS build
RUN apt-get update && apt-get install -y --no-install-recommends protobuf-compiler && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked --bin pandod --bin pando --bin git-remote-pando \
    && mkdir -p /out /data \
    && cp target/release/pandod target/release/pando target/release/git-remote-pando /out/

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=build /out/pandod /out/pando /out/git-remote-pando /usr/local/bin/
COPY --from=build --chown=65532:65532 /data /data
WORKDIR /data
VOLUME /data
# 7420 server, 7421 regional cache, 7422 node daemon, 9420 Prometheus metrics
EXPOSE 7420 7421 7422 9420
ENTRYPOINT ["/usr/local/bin/pandod"]
# Listens on the network, so it needs a token: `docker run -e PANDO_TOKENS="me:$(openssl rand -hex 24)" ...`
CMD ["server", "--listen", "0.0.0.0:7420", "--metadata", "sqlite:///data/meta.db", "--metrics", "0.0.0.0:9420"]
