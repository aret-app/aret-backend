//! Loading the aret-maps catalogue (aret-maps#13): `version.json` plus gzip `routes.geojson.gz` and
//! `places.geojson.gz`, published weekly at `https://aret-tiles.pages.dev/catalogue/`.
//!
//! Each update checks `version.json`; if the files' SHA-256 differ from what is loaded, it downloads
//! them, verifies the checksums and replaces all routes and places **in one transaction**: searches
//! see either the old catalogue or the new one, never half of it. A failed update leaves the old
//! catalogue in place.

use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use sqlx::types::Json;
use time::OffsetDateTime;

/// Environment variable: the catalogue's URL (ending in `/`) or a local folder with the files.
pub const CATALOGUE_ENV: &str = "PLACES_CATALOGUE";
/// Where aret-maps publishes the catalogue.
pub const DEFAULT_CATALOGUE: &str = "https://aret-tiles.pages.dev/catalogue/";

/// Features inserted per statement.
const BATCH: usize = 250;

/// Loading the catalogue failed; the catalogue already loaded (if any) is untouched.
#[must_use]
#[derive(Debug, thiserror::Error)]
pub enum CatalogueError {
    /// Download failed.
    #[error("downloading {0}: {1}")]
    Download(String, String),
    /// A file doesn't match the checksum in `version.json`.
    #[error("{0}: SHA-256 doesn't match version.json")]
    Checksum(String),
    /// A file isn't valid gzip or GeoJSON, or `version.json` is malformed.
    #[error("{0}: {1}")]
    Format(String, String),
    /// Writing to the database failed.
    #[error("database: {0}")]
    Database(#[from] sqlx::Error),
}

/// Where the catalogue comes from.
#[derive(Debug, Clone)]
pub enum Source {
    /// Published files, e.g. `https://aret-tiles.pages.dev/catalogue/`.
    Url(String, reqwest::Client),
    /// A local folder, e.g. `aret-maps/out/catalogue` (development, tests).
    Folder(PathBuf),
}

impl Source {
    /// A URL (`http…`) or a folder path.
    pub fn parse(location: &str) -> Result<Self, CatalogueError> {
        if location.starts_with("http://") || location.starts_with("https://") {
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(120))
                .user_agent("aret places-svc (+https://github.com/aret-app/aret-backend)")
                .build()
                .map_err(|error| {
                    CatalogueError::Download(location.to_owned(), error.to_string())
                })?;
            let base = if location.ends_with('/') {
                location.to_owned()
            } else {
                format!("{location}/")
            };
            Ok(Self::Url(base, client))
        } else {
            Ok(Self::Folder(PathBuf::from(location)))
        }
    }

    async fn read(&self, name: &str) -> Result<Vec<u8>, CatalogueError> {
        match self {
            Self::Url(base, client) => {
                let url = format!("{base}{name}");
                let fail = |error: reqwest::Error| {
                    CatalogueError::Download(url.clone(), error.to_string())
                };
                let response = client.get(&url).send().await.map_err(fail)?;
                let response = response.error_for_status().map_err(fail)?;
                Ok(response.bytes().await.map_err(fail)?.to_vec())
            }
            Self::Folder(folder) => {
                let path = folder.join(name);
                tokio::fs::read(&path).await.map_err(|error| {
                    CatalogueError::Download(path.display().to_string(), error.to_string())
                })
            }
        }
    }
}

/// `version.json`.
#[derive(Debug, Clone, Deserialize)]
pub struct Version {
    /// When aret-maps built the catalogue.
    #[serde(with = "time::serde::rfc3339")]
    pub built_at: OffsetDateTime,
    /// When the OpenStreetMap data was current.
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub osm_data_at: Option<OffsetDateTime>,
    /// The two files.
    pub files: Files,
}

/// The files listed in `version.json`.
#[derive(Debug, Clone, Deserialize)]
pub struct Files {
    /// `routes.geojson.gz`.
    pub routes: FileInfo,
    /// `places.geojson.gz`.
    pub places: FileInfo,
}

/// One file of the catalogue.
#[derive(Debug, Clone, Deserialize)]
pub struct FileInfo {
    /// File name relative to the catalogue.
    pub path: String,
    /// Number of features.
    pub count: usize,
    /// SHA-256 of the compressed file, hex.
    pub sha256: String,
}

/// Result of an update check.
#[derive(Debug, Clone, PartialEq)]
pub enum Update {
    /// The published catalogue is the one loaded.
    Unchanged,
    /// A new catalogue was loaded: (routes, places).
    Loaded(usize, usize),
}

/// Loads the catalogue from `source` if it differs from the one in the database.
pub async fn update(pool: &PgPool, source: &Source) -> Result<Update, CatalogueError> {
    let raw = source.read("version.json").await?;
    let version: Version = serde_json::from_slice(&raw)
        .map_err(|error| CatalogueError::Format("version.json".to_owned(), error.to_string()))?;

    let loaded: Option<(String, String)> =
        sqlx::query_as("SELECT routes_sha256, places_sha256 FROM catalogue")
            .fetch_optional(pool)
            .await?;
    if loaded.as_ref().is_some_and(|(routes, places)| {
        *routes == version.files.routes.sha256 && *places == version.files.places.sha256
    }) {
        return Ok(Update::Unchanged);
    }

    let routes = read_feature_collection(source, &version.files.routes).await?;
    let places = read_feature_collection(source, &version.files.places).await?;
    load(pool, &version, &routes, &places).await?;
    Ok(Update::Loaded(routes.len(), places.len()))
}

/// Downloads, verifies and parses one gzip GeoJSON file; returns its features.
async fn read_feature_collection(
    source: &Source,
    file: &FileInfo,
) -> Result<Vec<Value>, CatalogueError> {
    let compressed = source.read(&file.path).await?;
    let digest: String = Sha256::digest(&compressed)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if digest != file.sha256 {
        return Err(CatalogueError::Checksum(file.path.clone()));
    }
    let mut json = Vec::new();
    flate2::read::GzDecoder::new(compressed.as_slice())
        .read_to_end(&mut json)
        .map_err(|error| CatalogueError::Format(file.path.clone(), error.to_string()))?;
    let collection: Value = serde_json::from_slice(&json)
        .map_err(|error| CatalogueError::Format(file.path.clone(), error.to_string()))?;
    let Some(Value::Array(features)) = collection.get("features").cloned() else {
        return Err(CatalogueError::Format(
            file.path.clone(),
            "no features".to_owned(),
        ));
    };
    if features.len() != file.count {
        return Err(CatalogueError::Format(
            file.path.clone(),
            format!(
                "{} features, version.json says {}",
                features.len(),
                file.count
            ),
        ));
    }
    Ok(features)
}

/// Replaces everything with `routes` and `places`, in one transaction.
async fn load(
    pool: &PgPool,
    version: &Version,
    routes: &[Value],
    places: &[Value],
) -> Result<(), CatalogueError> {
    let mut transaction = pool.begin().await?;
    sqlx::query("DELETE FROM routes")
        .execute(&mut *transaction)
        .await?;
    sqlx::query("DELETE FROM places")
        .execute(&mut *transaction)
        .await?;

    for batch in routes.chunks(BATCH) {
        sqlx::query(
            "INSERT INTO routes (osm_id, name, ref, network, operator, cai, difficulty, \
                difficulty_source, from_name, to_name, osmc_symbol, length_m, pieces, missing_ways, \
                has_sub_routes, complete, ascent_m, descent_m, ele_min, ele_max, ele_start, ele_end, \
                duration_min, duration_reverse_min, profiles, geom) \
             SELECT (p->>'osm_id')::bigint, p->>'name', p->>'ref', p->>'network', p->>'operator', \
                (p->>'cai')::boolean, p->>'difficulty', p->>'difficulty_source', p->>'from', p->>'to', \
                p->>'osmc_symbol', (p->>'length_m')::double precision, (p->>'pieces')::integer, \
                (p->>'missing_ways')::integer, (p->>'has_sub_routes')::boolean, (p->>'complete')::boolean, \
                (p->>'ascent_m')::double precision, (p->>'descent_m')::double precision, \
                (p->>'ele_min')::double precision, (p->>'ele_max')::double precision, \
                (p->>'ele_start')::double precision, (p->>'ele_end')::double precision, \
                (p->>'duration_min')::integer, (p->>'duration_reverse_min')::integer, \
                NULLIF(p->'profiles', 'null'::jsonb), \
                ST_SetSRID(ST_Multi(ST_GeomFromGeoJSON(f->'geometry')), 4326) \
             FROM jsonb_array_elements($1) AS f, LATERAL (SELECT f->'properties' AS p) AS props",
        )
        .bind(Json(batch))
        .execute(&mut *transaction)
        .await?;
    }
    for batch in places.chunks(BATCH * 8) {
        sqlx::query(
            "INSERT INTO places (id, kind, type, name, alt_name, searchable, ele, ele_source, operator, \
                capacity, drinking_water, geom) \
             SELECT f->>'id', p->>'kind', p->>'type', p->>'name', p->>'alt_name', \
                (p->>'searchable')::boolean, (p->>'ele')::double precision, p->>'ele_source', \
                p->>'operator', p->>'capacity', p->>'drinking_water', \
                ST_SetSRID(ST_GeomFromGeoJSON(f->'geometry'), 4326) \
             FROM jsonb_array_elements($1) AS f, LATERAL (SELECT f->'properties' AS p) AS props",
        )
        .bind(Json(batch))
        .execute(&mut *transaction)
        .await?;
    }

    sqlx::query(
        "INSERT INTO catalogue (singleton, built_at, osm_data_at, routes_sha256, places_sha256) \
         VALUES (true, $1, $2, $3, $4) \
         ON CONFLICT (singleton) DO UPDATE SET built_at = $1, osm_data_at = $2, routes_sha256 = $3, \
            places_sha256 = $4, loaded_at = now()",
    )
    .bind(version.built_at)
    .bind(version.osm_data_at)
    .bind(&version.files.routes.sha256)
    .bind(&version.files.places.sha256)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(())
}

/// Checks for a new catalogue now and then every `interval`, forever.
pub async fn run_updates(pool: PgPool, source: Source, interval: Duration) {
    loop {
        match update(&pool, &source).await {
            Ok(Update::Loaded(routes, places)) => {
                tracing::info!(routes, places, "catalogue loaded");
            }
            Ok(Update::Unchanged) => tracing::debug!("catalogue unchanged"),
            Err(error) => tracing::warn!(%error, "catalogue update failed; keeping the loaded one"),
        }
        tokio::time::sleep(interval).await;
    }
}
