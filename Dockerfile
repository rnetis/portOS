# PortOS server — multi-stage build
FROM rust:1-slim AS build
WORKDIR /src
# rusqlite's bundled SQLite needs a C toolchain.
RUN apt-get update \
    && apt-get install -y --no-install-recommends build-essential pkg-config \
    && rm -rf /var/lib/apt/lists/*
COPY . .
ENV CARGO_TERM_COLOR=never
RUN cargo build --release --locked -p portos-api

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates wget \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /data \
    && chown 65534:65534 /data
COPY --from=build /src/target/release/portosd /usr/local/bin/portosd
ENV PORTOS_BIND_ADDR=0.0.0.0:8080 \
    PORTOS_DB_PATH=/data/portos.db \
    PORTOS_LOG_FORMAT=json \
    PORTOS_REQUIRE_SECRET=1
VOLUME ["/data"]
EXPOSE 8080
# PORTOS_JWT_SECRET must be provided by the orchestrator (>= 32 chars).
USER 65534:65534
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD wget -qO- http://127.0.0.1:8080/readyz || exit 1
ENTRYPOINT ["/usr/local/bin/portosd"]
CMD ["serve"]
