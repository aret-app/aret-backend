# aret-backend
Aret backend: Rust microservices (axum, sqlx, PostgreSQL/PostGIS)

One Cargo workspace, one binary and container per service
([ADR-0003](https://github.com/aret-app/aret-docs/blob/main/docs/adr/0003-service-granularity.md)).

```
crates/
  aret-common/   config from env, problem+json errors, logging, /health, graceful shutdown
  aret-auth/     AuthenticatedUser extractor (Kratos verification: #3)
  aret-events/   event types, transactional outbox, NATS JetStream publisher
  aret-testing/  throwaway test databases (dev-dependency only)
services/
  identity/      aret-identity  — adapter for Ory Kratos, user events (port 8081)
  tracks/        aret-tracks    — recorded tracks, PostGIS          (port 8082)
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
scripts/dev-env.sh            # once: private .env + session-JWT keys in dev/secrets/ (git-ignored)
docker compose up -d          # start (add --build after changing Rust code)
docker compose ps             # status; kratos-migrate and postgres-setup show "exited (0)"
docker compose logs -f kratos # follow one service's logs
docker compose down           # stop; `down -v` also deletes all local data
```

Postgres has one role per service ([dev/postgres/setup.sh](dev/postgres/setup.sh), run by the one-shot
`postgres-setup` service on every `up`, so a new service's role appears without wiping data):
`kratos` owns database `kratos`; `tracks` and `identity` own their schemas in database `aret`.
After pulling changes that add a variable, run `scripts/dev-env.sh` again: it adds the missing ones
and keeps your existing values.
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
- **Account:** sign up, verify the email with the code from Mailpit, log in, see the session, log out,
  delete the account
  (Kratos *browser* flows: cookies + CSRF, like a real web app; the iOS app uses the API flows)
- **tracks API:** *Get JWT* (Kratos `whoami?tokenize_as=aret`), then `GET /v1/tracks` with it → 200;
  without a JWT → 401
- **My tracks:** upload a GPX (try the synthetic `services/tracks/tests/fixtures/sample.gpx`), list your
  tracks, click one to draw it on the map, download it as GPX
- **Map:** the pilot tiles from `aret-tiles.pages.dev` with a first outdoor style (MapLibre GL JS 6), plus
  the **pre-built routes** (OpenStreetMap hiking relations, aret-app/aret-maps#4, rebuilt weekly): colour =
  difficulty (T, E, EE, EEA, unknown), dashed = incomplete (gaps never joined), CAI numbers along the
  lines; click a route for its details and a link to OpenStreetMap; a checkbox hides them
- **Requests:** every call the page makes, with its status

The gateway ([dev/gateway/Caddyfile](dev/gateway/Caddyfile)) puts everything on one address, so the
browser needs no CORS rules: `/kratos/*` → Kratos, `/api/identity/*` and `/api/tracks/*` → the
services. Kratos's public URL is therefore `http://127.0.0.1:8080/kratos/` (port 4433 still works
for curl and the iOS Simulator).

**With VS Code Live Server ("Go Live"):** open `dev/webapp/index.html` and click *Go Live*; the page
reloads on every save. It runs on `http://127.0.0.1:5500` and calls the gateway on `:8080`, which
allows exactly that origin with cookies. Use `127.0.0.1`, not `localhost`, or the browser treats the
login cookies as third-party and drops them. The stack must be running (`docker compose up -d`).

## Login check (session JWTs)
Services never call Kratos per request ([ADR-0008](https://github.com/aret-app/aret-docs/blob/main/docs/adr/0008-authentication.md)):

1. A logged-in client asks Kratos for a short-lived JWT: `GET /sessions/whoami?tokenize_as=aret`
   (cookie or session token) → field `tokenized`. Template in [dev/kratos/kratos.yml](dev/kratos/kratos.yml):
   ES256, 10 minutes, audience `aret-api` ([jwt-claims.jsonnet](dev/kratos/jwt-claims.jsonnet)).
2. It calls our APIs with `Authorization: Bearer <jwt>`.
3. `crates/aret-auth` verifies signature (public JWKS), `exp`/`nbf` (±30 s), issuer and audience, then gives
   the handler an `AuthenticatedUser { id }` (the Kratos identity id). Anything else → 401 problem+json.

The key pair comes from `scripts/dev-env.sh` (`dev/secrets/`, git-ignored): Kratos gets the private key,
services the public one. Production keys will be managed separately (never these dev keys).

## Tracks API (aret-tracks)
Every route needs a session JWT (see *Login check*). JSON, errors as problem+json.

| Route | What it does |
|---|---|
| `POST /v1/tracks` | Save a track: `{ id, title?, points: [{ lat, lon, ele, time }] }`. The **client generates `id`** (UUID v7): retrying the same upload returns **200** with the stored track instead of a duplicate; another user's id → 409. New → **201**. |
| `GET /v1/tracks?limit=&cursor=` | My tracks, newest first (20 per page, max 100); `next_cursor` until the last page |
| `GET /v1/tracks/{id}` | One track with all points; other users' tracks → 404 |
| `POST /v1/tracks/gpx?id=&title=` | Import a **GPX** file (body, `application/gpx+xml`): first `<trk>`, all segments joined; same rules and answers as `POST /v1/tracks`. Missing elevations are interpolated; points without time are rejected (a planned route, not a recording). |
| `GET /v1/tracks/{id}/gpx` | Export as **GPX 1.1** download (`aret-<date>-<id>.gpx`) |

The server computes **distance** (PostGIS, on the WGS 84 ellipsoid) and **ascent/descent** (changes under
3 m are noise). Points are stored as one PostGIS `LineStringZM` (lon, lat, elevation, time as M) in table
`tracks` ([migration](services/tracks/migrations/0001_tracks.sql)); migrations run at startup.
Limits: 2–100,000 points, time never going backwards, title ≤ 200 characters, body ≤ 16 MB.

**Tests against a real database:** `cargo test` creates a throwaway database per test on the local stack's
Postgres (password from your `.env`) and drops it afterwards. Without the stack running they print
`skipped: …` and pass. CI always runs them (`TEST_DATABASE_URL`, `REQUIRE_DB=1`).

## Events (NATS JetStream)
Services announce what happened as events on the `ARET_EVENTS` stream (subjects `aret.>`, kept 30
days; ADR-0009, [crates/aret-events](crates/aret-events/src/lib.rs)). Events carry **ids only**,
never personal data:

```json
{"id": "<uuid>", "occurred_at": "2026-11-26T10:00:00Z", "type": "user_created", "user_id": "<kratos identity id>"}
```

| Subject | When | Published by |
|---|---|---|
| `aret.user.created` | an account was registered (#12) | aret-identity |
| `aret.user.deleted` | the user deleted their account (#13): every service purges their data | aret-identity |
| `aret.purge.completed` | a service finished purging a deleted user (`"service": "tracks"`) | aret-tracks |

Services react through **durable JetStream consumers** (`aret_events::consumer`): JetStream remembers
what each one handled, redelivers on failure, and a new consumer first replays the stream. Handlers
must be idempotent.

**No event is lost:** a service writes the event to its `outbox` table in the same transaction as
its change; a relay publishes it and marks it sent. Event ids are deterministic (UUID v5 of
subject + user), so retries and duplicates are dropped by the outbox and by JetStream (1 h window).
Consumers must still tolerate a repeat.

**How aret-identity learns about new accounts:** Kratos calls `POST /internal/kratos/registration`
(secret header, body from [webhook-registration.jsonnet](dev/kratos/webhook-registration.jsonnet))
after saving the account, fire-and-forget, so sign-ups never wait for or fail on identity-svc.
Every 5 minutes a catch-up loop lists all accounts through the Kratos admin API and announces any the
web hook missed. (A blocking hook can't be used: Kratos runs it before the account has an id.)
`/internal/*` is never routed by the gateway.

Check: after a sign-up, `curl -s '127.0.0.1:8222/jsz?streams=true' | jq '.account_details[0].stream_detail[0].state.messages'`.

## Account deletion (GDPR right to erasure)
`DELETE /v1/account` on aret-identity (aret-backend#13); the test console has a **Delete my account** button.

1. Needs a session JWT from a login in the **last 15 minutes** (claim `authenticated_at`, added by
   [jwt-claims.jsonnet](dev/kratos/jwt-claims.jsonnet)); otherwise **403**. Answers **202**; repeating is harmless.
2. identity-svc marks the account deleted (the `users` row stays as an ids-only tombstone) and queues
   `user.deleted`, in one transaction.
3. It deletes the identity from Kratos **at once**: email, password and every session are gone, so
   nobody can sign in. If Kratos is unreachable, the catch-up loop retries.
4. tracks-svc deletes all the user's tracks, keeps a tombstone (`deleted_users`) and queues
   `purge.completed`, in one transaction. JWTs stay valid up to 10 minutes, so saving a track checks
   the tombstone; a per-user lock stops an upload racing the purge from leaving a track behind.
5. identity-svc records each `purge.completed` (`purges` table) and logs a warning when a service in
   `Service::ALL_WITH_USER_DATA` hasn't confirmed a day after the deletion. **A new service holding
   user data must consume `user.deleted`, answer `purge.completed` and be added to that list.**

Nothing is kept after deletion. Keeping anonymous statistics is an open decision (aret-app/aret-docs#25).

## Account recovery (forgot password)
Kratos's recovery flow with a one-time code ([dev/kratos/kratos.yml](dev/kratos/kratos.yml), aret-backend#15):

| Step | Browser (test console) | App (API flow, `/self-service/recovery/api`) |
|---|---|---|
| 1. Email | `POST recovery?flow=…` `{method: code, email, csrf_token}` → `sent_email` | same, no CSRF token |
| 2. Code | → `200 passed_challenge`, signed in by cookie; `continue_with: show_settings_ui` | → `200 passed_challenge`; `continue_with: set_ory_session_token` **+** `show_settings_ui` |
| 3. New password | `POST settings?flow=…` `{method: password, password}` → `success` | same, with `X-Session-Token` |

Rules (all tested end to end):
- **Privacy:** an address with no account gets the same answer and **no email**
  (`notify_unknown_recipients: false`), so nobody can probe who has an account.
- Codes expire after **15 minutes**; after **5 wrong codes** the flow restarts and a new code is needed.
- Setting the password needs a recent login (`privileged_session_max_age: 15m`, recovery provides it).
- A new password **signs out every other session** (`revoke_active_sessions` hook).
- `feature_flags.use_continue_with_transitions: true` is required: without it the app gets a browser
  redirect and **no session token** after recovery. Registration then needs the
  `show_verification_ui` hook to keep returning the verification flow.

## Configuration (environment variables)
| Variable | Default | Meaning |
|---|---|---|
| `ARET_HTTP_ADDR` | `127.0.0.1:<service port>` | Listen address (`0.0.0.0:8080` in containers) |
| `RUST_LOG` | `info` | Log level, e.g. `debug` or `aret_tracks=debug,info` |
| `ARET_LOG_FORMAT` | text | `json` for structured logs (set in containers) |
| `DATABASE_URL` | — (required by aret-tracks, aret-identity) | e.g. `postgres://tracks:<TRACKS_DB_PASSWORD>@127.0.0.1:5432/aret` |
| `NATS_URL` | — (required by aret-identity) | e.g. `nats://127.0.0.1:4222` |
| `NATS_URL` (aret-tracks) | — (required) | Same as above: tracks purges on `user.deleted` |
| `KRATOS_ADMIN_URL` | — (required by aret-identity) | Kratos admin API, e.g. `http://127.0.0.1:4434`; private network only |
| `ARET_KRATOS_WEBHOOK_SECRET` | — (required by aret-identity) | Shared secret of the Kratos web hook, ≥ 32 characters (`KRATOS_WEBHOOK_SECRET` in `.env`) |
| `ARET_JWKS_PATH` | `dev/secrets/jwks.public.json` | Public keys that verify Kratos session JWTs (services with login) |
| `ARET_JWT_ISSUER` | `http://127.0.0.1:8080/kratos/` | Expected `iss` of session JWTs: Kratos's public URL |

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
