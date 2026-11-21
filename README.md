# PortOS — Next-Generation Port Operations Management System

Rust implementation of the PortOS PRD (Phase 1 core platform): a bilingual
(English/Persian, RTL) API-first platform for vessel & berth planning, yard
management, gate automation, billing, KPIs and EDI integration.

| | |
|---|---|
| Language | Rust 1.100 (stable), edition 2021 |
| Crates | `portos-core` · `portos-i18n` · `portos-edi` · `portos-store` · `portos-api` |
| HTTP | axum 0.8, JWT (HS256) + Argon2id, RBAC |
| Storage | SQLite (WAL) with ordered migrations |
| Tests | 100 unit tests + end-to-end integration test (API flow, `cargo test --workspace`) |

## Quick start

```bash
# build & test
cargo test --workspace

# create the database, load realistic demo data, run the server
cargo run -p portos-api --bin portosd -- init-db
cargo run -p portos-api --bin portosd -- seed-demo
cargo run -p portos-api --bin portosd -- serve        # binds 0.0.0.0:8080
```

Demo logins (Argon2-hashed in the DB — rotate in production):
`admin/Admin!2026`, `planner/Planner!2026`, `gateclerk/Gate!2026`,
`customs/Customs!2026`, `finance/Finance!2026`, `agent/Agent!2026`.

```bash
TOKEN=$(curl -s localhost:8080/v1/auth/login -H 'content-type: application/json' \
  -d '{"username":"planner","password":"Planner!2026"}' | jq -r .access_token)

curl -s "localhost:8080/v1/kpi?hours=72&lang=fa&calendar=jalali" -H "authorization: Bearer $TOKEN"
curl -s "localhost:8080/v1/i18n/format-demo?date=2026-11-21&money=850000&currency=IRR&lang=fa"
curl -N "localhost:8080/v1/events/stream?token=$TOKEN"      # live SSE event feed (F-26)
```

- OpenAPI 3.0 spec: `GET /api-docs/openapi.json` (rendered index at `/api-docs`)
- Metrics: `GET /metrics` (Prometheus text format)
- Probes: `GET /healthz`, `GET /readyz`

## PRD coverage (Phase 1)

| Area | PRD | Status |
|---|---|---|
| Vessel calls, guarded lifecycle, ETA updates | F-1, F-3, F-5 | ✅ |
| Explainable berth allocation + human override | F-2, §6.6 | ✅ |
| Yard blocks/slots, segregation-aware placement | F-7, F-8, F-9 | ✅ |
| Reefer monitoring + auto alarms | F-10 | ✅ |
| Holds & container status | F-11 | ✅ |
| Truck appointments, queue visibility | F-17, F-20 | ✅ |
| Gate workflow (OCR/seal/release steps), passes, paperless EIR | F-18, F-19, F-21 | ✅ |
| Tariff engine (per-day/move/TEU/call/flat/kWh, tiers, free days, VAT) | F-27 | ✅ |
| Storage/reefer charge calculation, invoices, payments | F-28, F-29 | ✅ |
| KPI computations (moves/h, dwell, truck turnaround, anchorage wait) | F-31, F-32, §1.4 | ✅ |
| EDIFACT COPARN/CODECO/COARRI/BAPLIE parse + BAPLIE emit | I-2 | ✅ |
| SSE events, signed webhooks, durable outbox with backoff + dead-letter | F-26, I-1 | ✅ |
| JWT + RBAC (8 roles), audit trail | F-44, F-40 | ✅ |
| Bilingual catalogs, Jalali calendar, Persian numerals, bidi, RTL flag | L-1..L-10 | ✅ |
| Multi-currency incl. IRR + FX + Toman display | L-9 | ✅ |
| Phase 2/3 scope (community portal, automation agents, digital twin F-33, predictive maintenance F-16, customs integration F-24, CCTV F-37) | — | ⏳ roadmap (extension points prepared, not implemented) |

The live endpoint list and schemas are served by the running API at `/api-docs/openapi.json`.

## Architecture

```
portos-core   domain: ISO 6346/IMO/UN-LOCODE validation, berth & yard engines,
              billing engine, KPI math, events, money
portos-i18n   en/fa catalogs (JSON, hot-overridable), Jalali calendar,
              Persian numerals, bidi isolates, collation
portos-edi    EDIFACT tokenizer + 4 message mappers, round-trip tested
portos-store  SQLite WAL, migrations, repositories, hand-rolled pool
portos-api    axum server: REST /v1, auth/RBAC, problem+json, SSE, webhooks,
              metrics, rate limiting, OpenAPI, portosd CLI (serve/init-db/seed-demo)
```

Localization (PRD §3): storage and APIs are ISO 8601/UTC; display calendar
(`?calendar=jalali`, `X-Calendar`) and numerals (`?numerals=persian`,
`X-Numerals`) are per-request/user preference; language via `?lang=fa` /
`Accept-Language` / saved preference. Latin technical identifiers are wrapped
in Unicode directional isolates for RTL rendering (L-2). Catalogs can be
overridden at runtime via `PORTOS_I18N_EXTRA_DIRS` (L-10).

## Configuration

See `.env.example`. Key variables: `PORTOS_BIND_ADDR`, `PORTOS_DB_PATH`,
`PORTOS_JWT_SECRET` (set `PORTOS_REQUIRE_SECRET=1` in production to refuse the
dev default), `PORTOS_TENANT`, `PORTOS_CORS_ORIGINS`, `PORTOS_LOG_FORMAT`,
`PORTOS_I18N_EXTRA_DIRS`, `PORTOS_RATE_LIMIT_RPS`, `PORTOS_LOGIN_RATE_PER_MIN`.

## Deployment

A multi-stage `Dockerfile` (non-root user, writable `/data` volume, `readyz`
healthcheck) and `docker-compose.yml` are included. The container refuses to
start without `PORTOS_JWT_SECRET` (>= 32 chars) because it sets
`PORTOS_REQUIRE_SECRET=1`. Put the API behind TLS (reverse proxy) and enable
`PORTOS_TRUST_PROXY=1` only when the proxy is the sole ingress, otherwise
`X-Forwarded-For` is ignored for client-IP based rate limiting.

Operational notes:

- Every request receives an `X-Request-Id`; internal errors log the full cause
  under a short correlation id that is returned to the client instead of stack
  details.
- `/readyz` checks the database, `/healthz` only the process — use `readyz` for
  orchestrator probes.
- Migrations are applied automatically on startup (`schema_migrations` order).
- Webhooks are delivered from a durable outbox in the database with exponential
  backoff (8 attempts) and a terminal dead-letter state; restarting the process
  does not lose queued deliveries.
- `PORTOS_CORS_ORIGINS` should list explicit origins in production; `*` is
  intended for local development only.

## Known limitations (v1.0.0)

- SQLite is the storage engine (single writer); PostgreSQL is the planned
  scaling path — the repository layer is isolated behind `portos-store`.
- SQLite uses a single writer; write throughput is bounded by one connection
  at a time. The repository layer is isolated in `portos-store` so PostgreSQL
  can be added without touching domain or route code.
- SSE event streams and the outbox dispatcher are single-process; running
  multiple API replicas against one SQLite file is not supported.

## License

MIT
