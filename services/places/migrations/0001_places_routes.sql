-- places-svc (aret-backend#24): places and pre-built routes from the aret-maps catalogue (ADR-0017).
-- Read-only reference data (OpenStreetMap, ODbL), replaced as a whole by each import.
-- Runs as role `places`, whose search_path puts the tables in schema `places`; PostGIS and the
-- search extensions live in `public` (created by dev/postgres/setup.sh as superuser).

CREATE EXTENSION IF NOT EXISTS pg_trgm;
CREATE EXTENSION IF NOT EXISTS unaccent;

-- Text as compared in search: lower case, accents removed ("Sant'Anna" ~ "sant anna").
-- unaccent() alone isn't IMMUTABLE (it reads a dictionary setting), so indexes need this wrapper.
CREATE FUNCTION search_text(value text) RETURNS text
    LANGUAGE sql IMMUTABLE STRICT PARALLEL SAFE
    AS $$ SELECT lower(public.unaccent('public.unaccent'::regdictionary, value)) $$;

CREATE TABLE places (
    id              text PRIMARY KEY,            -- 'node/123' or 'way/456'
    kind            text NOT NULL,               -- peak, pass, hut, water, trailhead, settlement
    type            text NOT NULL,               -- e.g. alpine_hut, spring, village
    name            text,
    alt_name        text,
    searchable      boolean NOT NULL,            -- has a name
    ele             double precision,
    ele_source      text,                        -- 'osm' tag or 'dem' (TINITALY)
    operator        text,
    capacity        text,
    drinking_water  text,
    geom            geometry(Point, 4326) NOT NULL
);
CREATE INDEX places_name_search_idx ON places USING gin (search_text(name) gin_trgm_ops) WHERE searchable;
CREATE INDEX places_alt_name_search_idx ON places USING gin (search_text(alt_name) gin_trgm_ops)
    WHERE alt_name IS NOT NULL;
CREATE INDEX places_geom_idx ON places USING gist (geom);

CREATE TABLE routes (
    osm_id                bigint PRIMARY KEY,     -- OpenStreetMap relation id
    name                  text,
    ref                   text,                   -- CAI number
    network               text,
    operator              text,
    cai                   boolean NOT NULL,
    difficulty            text NOT NULL,          -- from tags only; 'unknown' otherwise
    difficulty_source     text,
    from_name             text,
    to_name               text,
    osmc_symbol           text,
    length_m              double precision NOT NULL,
    pieces                integer NOT NULL,
    missing_ways          integer NOT NULL,
    has_sub_routes        boolean NOT NULL,
    complete              boolean NOT NULL,
    ascent_m              double precision,
    descent_m             double precision,
    ele_min               double precision,
    ele_max               double precision,
    ele_start             double precision,
    ele_end               double precision,
    duration_min          integer,
    duration_reverse_min  integer,
    profiles              jsonb,                  -- per line: [[distance m, elevation m], …]
    geom                  geometry(MultiLineString, 4326) NOT NULL
);
-- Distances in metres ("routes within 200 m of a summit")
CREATE INDEX routes_geog_idx ON routes USING gist ((geom::geography));

-- The catalogue build currently loaded (a single row)
CREATE TABLE catalogue (
    singleton      boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    built_at       timestamptz NOT NULL,
    osm_data_at    timestamptz,
    routes_sha256  text NOT NULL,
    places_sha256  text NOT NULL,
    loaded_at      timestamptz NOT NULL DEFAULT now()
);
