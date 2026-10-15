# syntax=docker/dockerfile:1
# Multi-stage build: compile with the full toolchain, ship a ~30 MB distroless image
# running as a non-root user.

FROM rust:1-bookworm AS build
RUN apt-get update && apt-get install -y --no-install-recommends protobuf-compiler && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked --bin sapwood --bin sap --bin git-remote-sapwood \
    && mkdir -p /out /data \
    && cp target/release/sapwood target/release/sap target/release/git-remote-sapwood /out/

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=build /out/sapwood /out/sap /out/git-remote-sapwood /usr/local/bin/
COPY --from=build --chown=65532:65532 /data /data
WORKDIR /data
VOLUME /data
# 7420 server, 7421 regional cache, 7422 node daemon, 9420 Prometheus metrics
EXPOSE 7420 7421 7422 9420
ENTRYPOINT ["/usr/local/bin/sapwood"]
CMD ["server", "--listen", "0.0.0.0:7420", "--metadata", "sqlite:///data/meta.db", "--metrics", "0.0.0.0:9420"]
