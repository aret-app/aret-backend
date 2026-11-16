# aret-backend
Aret backend: Rust microservices (axum, sqlx, PostgreSQL/PostGIS)

One Cargo workspace, one binary and container per service
([ADR-0003](https://github.com/aret-app/aret-docs/blob/main/docs/adr/0003-service-granularity.md)).

```
crates/
  aret-common/   config from env, problem+json errors, logging, /health, graceful shutdown
  aret-auth/     AuthenticatedUser extractor (Kratos verification: #3)
services/
  identity/      aret-identity  — adapter for Ory Kratos           (port 8081)
  tracks/        aret-tracks    — recorded tracks and sync          (port 8082)
```

## Requirements
- [rustup](https://rustup.rs). The Rust version is pinned in `rust-toolchain.toml` and installed automatically.
- `cargo-deny` (`brew install cargo-deny`) for license and security checks
- Docker (e.g. Colima) only to build container images

## Everyday commands
```bash
cargo build                                  # compile everything
cargo test                                   # run all tests
cargo fmt                                    # format code (CI runs: cargo fmt --check)
cargo clippy --all-targets -- -D warnings    # lints; warnings fail CI
cargo deny check                             # licenses, advisories, sources
```

Run a service (Ctrl-C to stop):
```bash
cargo run -p aret-tracks
curl -s localhost:8082/health                # {"status":"ok","service":"tracks","version":"0.1.0"}
curl -si localhost:8082/v1/tracks            # 401 problem+json: login required
```

## Code style
We follow the [Google Rust style guide](https://google.github.io/styleguide/rustguide.html)
(the general rules, not the Chrome/Fuchsia-specific ones). Formatting and most rules are enforced by
`cargo fmt`, `clippy.toml` and the lints in `Cargo.toml`. Notes on the nightly-only formatting rules
we follow by hand are in [rustfmt.toml](rustfmt.toml).

## Local stack (Docker Compose)
Everything the services need, on your computer, with one command ([compose.yaml](compose.yaml)):

| Service | Image | Address (this computer only) |
|---|---|---|
| PostgreSQL 18 + PostGIS 3.7 | built from [dev/postgres/Dockerfile](dev/postgres/Dockerfile) (official `postgres:18` + PGDG PostGIS; works on ARM) | `127.0.0.1:5432` |
| Ory Kratos (accounts, ADR-0008) | `oryd/kratos:v26.2.0` | public `:4433`, admin `:4434` |
| NATS JetStream (events, ADR-0009) | `nats:2.15-alpine` | `:4222`, monitoring `:8222` |
| Valkey (cache) | `valkey/valkey:9.0-alpine` | `:6379` |
| RustFS (S3-compatible files; MinIO is discontinued) | `rustfs/rustfs:1.0.5` | S3 `:9000`, console http://127.0.0.1:9001/rustfs/console/ |
| Mailpit (catches all emails) | `axllent/mailpit:v1.33` | web UI `:8025` |
| aret-identity / aret-tracks | built from this repo | `:8081` / `:8082` |
| Gateway + test web app (Caddy) | `caddy:2.11-alpine` | **http://127.0.0.1:8080** |

Requirements: Docker with Compose (e.g. `brew install colima docker docker-compose`, then `colima start`).

```bash
scripts/dev-env.sh            # once: creates your private .env with random passwords
docker compose up -d          # start (add --build after changing Rust code)
docker compose ps             # status; kratos-migrate should show "exited (0)"
docker compose logs -f kratos # follow one service's logs
docker compose down           # stop; `down -v` also deletes all local data
```

Postgres creates one role per service on first start ([dev/postgres/20-aret.sh](dev/postgres/20-aret.sh)):
`kratos` owns database `kratos`; `tracks` owns schema `tracks` in database `aret`.
Kratos config: [dev/kratos/](dev/kratos/) — accounts hold only a verified email (GDPR data minimisation).

Try a registration the way the iOS app will (Kratos API flow), then open Mailpit at
http://127.0.0.1:8025 to see the verification email:
```bash
FLOW=$(curl -s 127.0.0.1:4433/self-service/registration/api | jq -r .id)
curl -s -X POST "127.0.0.1:4433/self-service/registration?flow=$FLOW" \
  -H 'Content-Type: application/json' \
  -d "{\"method\":\"password\",\"traits\":{\"email\":\"hiker@example.com\"},\"password\":\"$(openssl rand -base64 18)\"}" \
  | jq '{identity: .identity.id, email: .identity.traits.email, got_session_token: (.session_token != null)}'
```

## Test web app (local only)
**http://127.0.0.1:8080**, started with the stack: a small console to try the backend in a browser
until the iOS app talks to it. Plain HTML + JavaScript in [dev/webapp/](dev/webapp/), no build step:
edit a file and reload the page.

- **Services:** health of Kratos, aret-identity and aret-tracks
- **Account:** sign up, verify the email with the code from Mailpit, log in, see the session, log out
  (Kratos *browser* flows: cookies + CSRF, like a real web app; the iOS app uses the API flows)
- **tracks API:** calls `GET /v1/tracks` (401 until aret-backend#3 verifies Kratos sessions)
- **Map:** the pilot tiles from `aret-tiles.pages.dev` with a first outdoor style (MapLibre GL JS 6)
- **Requests:** every call the page makes, with its status

The gateway ([dev/gateway/Caddyfile](dev/gateway/Caddyfile)) puts everything on one address, so the
browser needs no CORS rules: `/kratos/*` → Kratos, `/api/identity/*` and `/api/tracks/*` → the
services. Kratos's public URL is therefore `http://127.0.0.1:8080/kratos/` (port 4433 still works
for curl and the iOS Simulator).

**With VS Code Live Server ("Go Live"):** open `dev/webapp/index.html` and click *Go Live*; the page
reloads on every save. It runs on `http://127.0.0.1:5500` and calls the gateway on `:8080`, which
allows exactly that origin with cookies. Use `127.0.0.1`, not `localhost`, or the browser treats the
login cookies as third-party and drops them. The stack must be running (`docker compose up -d`).

## Configuration (environment variables)
| Variable | Default | Meaning |
|---|---|---|
| `ARET_HTTP_ADDR` | `127.0.0.1:<service port>` | Listen address (`0.0.0.0:8080` in containers) |
| `RUST_LOG` | `info` | Log level, e.g. `debug` or `aret_tracks=debug,info` |
| `ARET_LOG_FORMAT` | text | `json` for structured logs (set in containers) |

Never commit secrets: use a git-ignored `.env` and commit only `.env.example`.

## Container images
```bash
docker build --build-arg SERVICE=aret-tracks -t aret-tracks .
docker run --rm -p 8082:8080 aret-tracks
```
Static (musl) binary on `gcr.io/distroless/static-debian12:nonroot`: no shell, non-root, a few MB.

## Errors
Handlers return `aret_common::error::ApiError`, sent as RFC 9457 `application/problem+json`:
```json
{ "type": "about:blank", "title": "Unauthorized", "status": 401, "detail": "missing bearer token" }
```

## License
AGPL-3.0 ([ADR-0011](https://github.com/aret-app/aret-docs/blob/main/docs/adr/0011-licenses.md)).
