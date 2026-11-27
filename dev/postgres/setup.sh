#!/bin/sh
# Local development database setup: one login and schema per service (spec §7.2), so each
# service can only touch its own data.
#
#   kratos   → own database `kratos` (Kratos manages its tables, ADR-0008)
#   tracks   → schema `tracks` in database `aret`, with PostGIS available
#   identity → schema `identity` in database `aret` (event outbox, aret-backend#12)
#
# Idempotent: it runs when the postgres volume is first created (docker-entrypoint-initdb.d) AND
# on every `docker compose up` (one-shot service `postgres-setup`), so a new service gets its
# login without wiping local data, and passwords follow your .env.
#
# Passwords are passed as psql variables (:'name'), so psql quotes them; never paste them into SQL.
set -eu

psql -v ON_ERROR_STOP=1 --username "$POSTGRES_USER" --dbname "$POSTGRES_DB" \
  -v kratos_password="$KRATOS_DB_PASSWORD" \
  -v tracks_password="$TRACKS_DB_PASSWORD" \
  -v identity_password="$IDENTITY_DB_PASSWORD" -v places_password="$PLACES_DB_PASSWORD" <<'EOSQL'
SELECT 'CREATE ROLE kratos LOGIN' WHERE NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'kratos')\gexec
ALTER ROLE kratos PASSWORD :'kratos_password';
SELECT 'CREATE DATABASE kratos OWNER kratos'
  WHERE NOT EXISTS (SELECT FROM pg_database WHERE datname = 'kratos')\gexec

CREATE EXTENSION IF NOT EXISTS postgis;

SELECT 'CREATE ROLE tracks LOGIN' WHERE NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'tracks')\gexec
ALTER ROLE tracks PASSWORD :'tracks_password';
CREATE SCHEMA IF NOT EXISTS tracks AUTHORIZATION tracks;
ALTER ROLE tracks SET search_path = tracks, public;

SELECT 'CREATE ROLE identity LOGIN' WHERE NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'identity')\gexec
ALTER ROLE identity PASSWORD :'identity_password';
CREATE SCHEMA IF NOT EXISTS identity AUTHORIZATION identity;
ALTER ROLE identity SET search_path = identity, public;

-- places-svc (aret-backend#24): typo- and accent-tolerant name search
CREATE EXTENSION IF NOT EXISTS pg_trgm;
CREATE EXTENSION IF NOT EXISTS unaccent;
SELECT 'CREATE ROLE places LOGIN' WHERE NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'places')\gexec
ALTER ROLE places PASSWORD :'places_password';
CREATE SCHEMA IF NOT EXISTS places AUTHORIZATION places;
ALTER ROLE places SET search_path = places, public;
EOSQL

echo "aret: database roles and schemas are set up (kratos, tracks, identity)"
