# PostgreSQL 18 + PostGIS 3, for every CPU architecture (Apple Silicon, ARM servers, x86).
# The official postgis/postgis image is amd64-only; the official postgres image is multi-arch and
# already has the PostgreSQL project's package repository (PGDG) configured, which ships PostGIS.
FROM postgres:18-trixie

RUN apt-get update \
 && apt-get install -y --no-install-recommends postgresql-18-postgis-3 postgresql-18-postgis-3-scripts \
 && rm -rf /var/lib/apt/lists/*
