# syntax=docker/dockerfile:1
# One Dockerfile for every service:
#   docker build --build-arg SERVICE=aret-identity -t aret-identity .
#   docker build --build-arg SERVICE=aret-tracks   -t aret-tracks .
# PROFILE: `release` (default; CI and production) or `docker-dev` (local Compose: builds much
# faster, see Cargo.toml).

# ---- Stage 1: compile a static binary (musl), so the runtime image needs no libraries
FROM rust:1.100-alpine AS build
RUN apk add --no-cache musl-dev
WORKDIR /src
COPY . .
ARG SERVICE
ARG PROFILE=release
RUN test -n "$SERVICE" || (echo "Pass --build-arg SERVICE=aret-identity (or aret-tracks)" && exit 1)
# Cache mounts (BuildKit) keep downloaded crates and compiled dependencies between builds, so a
# rebuild only recompiles what changed. They are never part of the image; the binary is copied out
# in the same step. All services share one cache, so dependencies compile once; `sharing=locked`
# makes parallel builds take turns instead of corrupting it.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,target=/src/target,sharing=locked \
    cargo build --locked --profile "$PROFILE" --package "$SERVICE" \
 && cp "target/$PROFILE/$SERVICE" /service

# ---- Stage 2: minimal runtime image — no shell, no package manager, runs as non-root
FROM gcr.io/distroless/static-debian12:nonroot
COPY --from=build /service /service
ENV ARET_HTTP_ADDR=0.0.0.0:8080 \
    ARET_LOG_FORMAT=json
EXPOSE 8080
ENTRYPOINT ["/service"]
