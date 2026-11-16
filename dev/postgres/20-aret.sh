#!/bin/sh
# Local development database setup. Runs ONCE, when the postgres volume is first created
# (to re-run: docker compose down -v && docker compose up -d — this deletes local data).
#
# One database role per service (spec §7.2): each service can only touch its own schema.
#   kratos  → own database `kratos` (Kratos manages its tables, ADR-0008)
#   tracks  → schema `tracks` in database `aret`, with PostGIS available
set -eu

psql -v ON_ERROR_STOP=1 --username "$POSTGRES_USER" --dbname "$POSTGRES_DB" <<EOSQL
CREATE ROLE kratos LOGIN PASSWORD '${KRATOS_DB_PASSWORD}';
CREATE DATABASE kratos OWNER kratos;

CREATE EXTENSION IF NOT EXISTS postgis;
CREATE ROLE tracks LOGIN PASSWORD '${TRACKS_DB_PASSWORD}';
CREATE SCHEMA tracks AUTHORIZATION tracks;
ALTER ROLE tracks SET search_path = tracks, public;
EOSQL

echo "aret: created roles kratos, tracks; database kratos; schema tracks"
