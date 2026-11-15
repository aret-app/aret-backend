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
