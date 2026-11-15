# One Dockerfile for every service:
#   docker build --build-arg SERVICE=aret-identity -t aret-identity .
#   docker build --build-arg SERVICE=aret-tracks   -t aret-tracks .

# ---- Stage 1: compile a static binary (musl), so the runtime image needs no libraries
FROM rust:1.100-alpine AS build
RUN apk add --no-cache musl-dev
WORKDIR /src
COPY . .
ARG SERVICE
RUN test -n "$SERVICE" || (echo "Pass --build-arg SERVICE=aret-identity (or aret-tracks)" && exit 1)
RUN cargo build --release --locked --package "$SERVICE" \
 && cp "target/release/$SERVICE" /service

# ---- Stage 2: minimal runtime image — no shell, no package manager, runs as non-root
FROM gcr.io/distroless/static-debian12:nonroot
COPY --from=build /service /service
ENV ARET_HTTP_ADDR=0.0.0.0:8080 \
    ARET_LOG_FORMAT=json
EXPOSE 8080
ENTRYPOINT ["/service"]
