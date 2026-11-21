# PortOS server — multi-stage build
FROM rust:1-slim AS build
WORKDIR /src
COPY . .
ENV CARGO_TERM_COLOR=never
RUN cargo build --release -p portos-api

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/portosd /usr/local/bin/portosd
ENV PORTOS_BIND_ADDR=0.0.0.0:8080 \
    PORTOS_DB_PATH=/data/portos.db \
    PORTOS_LOG_FORMAT=json \
    PORTOS_REQUIRE_SECRET=1
VOLUME ["/data"]
EXPOSE 8080
# PORTOS_JWT_SECRET must be provided by the orchestrator
USER 65534:65534
ENTRYPOINT ["/usr/local/bin/portosd"]
CMD ["serve"]
