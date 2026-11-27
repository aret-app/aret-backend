//! Public search API (no login: OpenStreetMap reference data, ODbL). Every answer carries the
//! credit, the licence and the date of the data.
//!
//! - `GET /v1/places?q=&limit=`: places by name, typo- and accent-tolerant, peaks favoured
//! - `GET /v1/routes?near=<place id>` or `?lat=&lon=` (`&radius_m=`): routes that reach the point
//!   (within 200 m) first, then routes nearby
//! - `GET /v1/routes/{osm_id}`: one route with geometry and elevation profile
//! - `GET /v1/routes/{osm_id}/gpx`: the route as GPX 1.1

use aret_common::error::ApiError;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_TYPE};
use axum::response::IntoResponse;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::PgPool;
use time::OffsetDateTime;

/// A route counts as reaching a place within this distance.
pub const REACH_M: f64 = 200.0;
/// Default and largest search radius for routes near a place.
const DEFAULT_RADIUS_M: f64 = 3_000.0;
const MAX_RADIUS_M: f64 = 10_000.0;

const ATTRIBUTION: &str = "© OpenStreetMap contributors";
const LICENSE: &str = "ODbL-1.0";

/// Credit, licence and data dates for a response; 503 before the first catalogue is loaded.
async fn data_info(pool: &PgPool) -> Result<Value, ApiError> {
    let row: Option<(OffsetDateTime, Option<OffsetDateTime>)> =
        sqlx::query_as("SELECT built_at, osm_data_at FROM catalogue")
            .fetch_optional(pool)
            .await
            .map_err(database_error)?;
    let Some((built_at, osm_data_at)) = row else {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "the route catalogue is still loading; try again in a minute",
        ));
    };
    let rfc3339 = |at: OffsetDateTime| {
        at.format(&time::format_description::well_known::Rfc3339)
            .ok()
    };
    Ok(json!({
        "attribution": ATTRIBUTION,
        "license": LICENSE,
        "catalogue_built_at": rfc3339(built_at),
        "osm_data_at": osm_data_at.and_then(rfc3339),
    }))
}

fn database_error(error: sqlx::Error) -> ApiError {
    tracing::error!(%error, "database error");
    ApiError::internal()
}

/// A place found by name: id, kind, type, name, elevation, longitude, latitude, score.
type PlaceRow = (
    String,
    String,
    String,
    Option<String>,
    Option<f64>,
    f64,
    f64,
    f64,
);

/// Query of `GET /v1/places`.
#[derive(Debug, Deserialize)]
pub struct PlaceQuery {
    /// What the user typed.
    pub q: String,
    /// Results, 1–50 (default 10).
    pub limit: Option<i64>,
}

/// `GET /v1/places?q=`.
pub async fn search_places(
    State(pool): State<PgPool>,
    Query(query): Query<PlaceQuery>,
) -> Result<Json<Value>, ApiError> {
    let text = query.q.trim();
    if !(2..=100).contains(&text.chars().count()) {
        return Err(ApiError::bad_request("q must be 2 to 100 characters"));
    }
    let limit = query.limit.unwrap_or(10).clamp(1, 50);
    let data = data_info(&pool).await?;

    // Similarity of the whole name, or of the words typed within the name; exact and prefix
    // matches first; among similar matches, peaks, passes and huts before villages and springs
    let rows: Vec<PlaceRow> = sqlx::query_as(
        "WITH q AS (SELECT search_text($1) AS t) \
         SELECT id, kind, type, name, ele, ST_X(geom), ST_Y(geom), \
            GREATEST(similarity(search_text(name), q.t), word_similarity(q.t, search_text(name)), \
                     COALESCE(similarity(search_text(alt_name), q.t), 0), \
                     COALESCE(word_similarity(q.t, search_text(alt_name)), 0)) \
            + CASE WHEN search_text(name) = q.t THEN 1.0 \
                   WHEN search_text(name) LIKE q.t || '%' THEN 0.3 ELSE 0 END \
            + CASE kind WHEN 'peak' THEN 0.10 WHEN 'pass' THEN 0.08 WHEN 'hut' THEN 0.08 \
                        WHEN 'settlement' THEN 0.05 ELSE 0 END AS score \
         FROM places, q \
         WHERE searchable AND (search_text(name) % q.t OR q.t <% search_text(name) \
                               OR search_text(alt_name) % q.t OR q.t <% search_text(alt_name)) \
         ORDER BY score DESC, name \
         LIMIT $2",
    )
    .bind(text)
    .bind(limit)
    .fetch_all(&pool)
    .await
    .map_err(database_error)?;

    let places: Vec<Value> = rows
        .into_iter()
        .map(|(id, kind, place_type, name, ele, lon, lat, score)| {
            json!({
                "id": id,
                "kind": kind,
                "type": place_type,
                "name": name,
                "ele": ele,
                "position": [lon, lat],
                "score": (score * 1000.0).round() / 1000.0,
            })
        })
        .collect();
    Ok(Json(json!({ "places": places, "data": data })))
}

/// Query of `GET /v1/routes`.
#[derive(Debug, Deserialize)]
pub struct NearQuery {
    /// A place id from `/v1/places`, e.g. `node/3140315349`.
    pub near: Option<String>,
    /// Or a position.
    pub lat: Option<f64>,
    /// Or a position.
    pub lon: Option<f64>,
    /// How far to look, in metres (default 3,000, at most 10,000).
    pub radius_m: Option<f64>,
}

/// Columns of a route summary, in the order [`summary`] reads them.
const SUMMARY_COLUMNS: &str = "osm_id, name, ref, network, cai, difficulty, length_m, complete, \
     ascent_m, descent_m, ele_max, ele_start, duration_min, duration_reverse_min, \
     ST_X(ST_StartPoint(ST_GeometryN(geom, 1))) AS start_lon, \
     ST_Y(ST_StartPoint(ST_GeometryN(geom, 1))) AS start_lat";

/// A route's summary from a row selected with [`SUMMARY_COLUMNS`].
fn summary(row: &sqlx::postgres::PgRow) -> Result<Value, sqlx::Error> {
    use sqlx::Row;

    Ok(json!({
        "osm_id": row.try_get::<i64, _>("osm_id")?,
        "name": row.try_get::<Option<String>, _>("name")?,
        "ref": row.try_get::<Option<String>, _>("ref")?,
        "network": row.try_get::<Option<String>, _>("network")?,
        "cai": row.try_get::<bool, _>("cai")?,
        "difficulty": row.try_get::<String, _>("difficulty")?,
        "length_m": row.try_get::<f64, _>("length_m")?,
        "complete": row.try_get::<bool, _>("complete")?,
        "ascent_m": row.try_get::<Option<f64>, _>("ascent_m")?,
        "descent_m": row.try_get::<Option<f64>, _>("descent_m")?,
        "ele_max": row.try_get::<Option<f64>, _>("ele_max")?,
        "ele_start": row.try_get::<Option<f64>, _>("ele_start")?,
        "duration_min": row.try_get::<Option<i32>, _>("duration_min")?,
        "duration_reverse_min": row.try_get::<Option<i32>, _>("duration_reverse_min")?,
        "start": [row.try_get::<f64, _>("start_lon")?, row.try_get::<f64, _>("start_lat")?],
    }))
}

/// `GET /v1/routes?near=` or `?lat=&lon=`.
pub async fn routes_near(
    State(pool): State<PgPool>,
    Query(query): Query<NearQuery>,
) -> Result<Json<Value>, ApiError> {
    use sqlx::Row;

    let data = data_info(&pool).await?;
    let radius = query
        .radius_m
        .unwrap_or(DEFAULT_RADIUS_M)
        .clamp(REACH_M, MAX_RADIUS_M);
    let (point, place) = match (&query.near, query.lat, query.lon) {
        (Some(id), _, _) => {
            let row: Option<(f64, f64, Option<String>, String)> = sqlx::query_as(
                "SELECT ST_X(geom), ST_Y(geom), name, kind FROM places WHERE id = $1",
            )
            .bind(id)
            .fetch_optional(&pool)
            .await
            .map_err(database_error)?;
            let Some((lon, lat, name, kind)) = row else {
                return Err(ApiError::not_found("no such place"));
            };
            (
                (lon, lat),
                json!({ "id": id, "name": name, "kind": kind, "position": [lon, lat] }),
            )
        }
        (None, Some(lat), Some(lon))
            if (-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon) =>
        {
            ((lon, lat), json!({ "position": [lon, lat] }))
        }
        _ => {
            return Err(ApiError::bad_request(
                "give near=<place id> or lat= and lon= (WGS 84 degrees)",
            ));
        }
    };

    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "WITH p AS (SELECT ST_SetSRID(ST_MakePoint($1, $2), 4326)::geography AS g) \
         SELECT {SUMMARY_COLUMNS}, ST_Distance(geom::geography, p.g) AS distance_m \
         FROM routes, p \
         WHERE ST_DWithin(geom::geography, p.g, $3) \
         ORDER BY ST_Distance(geom::geography, p.g) > $4, duration_min NULLS LAST, length_m \
         LIMIT 50"
    )))
    .bind(point.0)
    .bind(point.1)
    .bind(radius)
    .bind(REACH_M)
    .fetch_all(&pool)
    .await
    .map_err(database_error)?;

    let routes = rows
        .iter()
        .map(|row| {
            let distance: f64 = row.try_get("distance_m")?;
            let mut route = summary(row)?;
            route["relation"] = json!(if distance <= REACH_M {
                "reaches"
            } else {
                "nearby"
            });
            route["distance_m"] = json!(distance.round());
            Ok(route)
        })
        .collect::<Result<Vec<Value>, sqlx::Error>>()
        .map_err(database_error)?;
    Ok(Json(
        json!({ "place": place, "radius_m": radius, "routes": routes, "data": data }),
    ))
}

/// `GET /v1/routes/{osm_id}`: summary, all fields, geometry (GeoJSON) and elevation profiles.
pub async fn route_detail(
    State(pool): State<PgPool>,
    Path(osm_id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    use sqlx::Row;

    let data = data_info(&pool).await?;
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {SUMMARY_COLUMNS}, operator, difficulty_source, from_name, to_name, osmc_symbol, \
            pieces, missing_ways, has_sub_routes, descent_m, ele_min, ele_end, profiles, \
            ST_AsGeoJSON(geom, 6)::jsonb AS geometry \
         FROM routes WHERE osm_id = $1"
    )))
    .bind(osm_id)
    .fetch_optional(&pool)
    .await
    .map_err(database_error)?
    .ok_or_else(|| ApiError::not_found("no such route"))?;

    let mut route = summary(&row).map_err(database_error)?;
    let extra = |row: &sqlx::postgres::PgRow| -> Result<Value, sqlx::Error> {
        Ok(json!({
            "operator": row.try_get::<Option<String>, _>("operator")?,
            "difficulty_source": row.try_get::<Option<String>, _>("difficulty_source")?,
            "from": row.try_get::<Option<String>, _>("from_name")?,
            "to": row.try_get::<Option<String>, _>("to_name")?,
            "osmc_symbol": row.try_get::<Option<String>, _>("osmc_symbol")?,
            "pieces": row.try_get::<i32, _>("pieces")?,
            "missing_ways": row.try_get::<i32, _>("missing_ways")?,
            "has_sub_routes": row.try_get::<bool, _>("has_sub_routes")?,
            "ele_min": row.try_get::<Option<f64>, _>("ele_min")?,
            "ele_end": row.try_get::<Option<f64>, _>("ele_end")?,
            "profiles": row.try_get::<Option<Value>, _>("profiles")?,
            "geometry": row.try_get::<Value, _>("geometry")?,
            "osm_url": format!("https://www.openstreetmap.org/relation/{osm_id}"),
        }))
    };
    let Value::Object(fields) = extra(&row).map_err(database_error)? else {
        return Err(ApiError::internal());
    };
    if let Value::Object(target) = &mut route {
        target.extend(fields);
    }
    Ok(Json(json!({ "route": route, "data": data })))
}

/// `GET /v1/routes/{osm_id}/gpx`: one track, one segment per line (a route with gaps keeps them).
pub async fn route_gpx(
    State(pool): State<PgPool>,
    Path(osm_id): Path<i64>,
) -> Result<impl IntoResponse, ApiError> {
    data_info(&pool).await?;
    let row: Option<(Option<String>, Option<String>, Value)> = sqlx::query_as(
        "SELECT name, ref, ST_AsGeoJSON(geom, 6)::jsonb FROM routes WHERE osm_id = $1",
    )
    .bind(osm_id)
    .fetch_optional(&pool)
    .await
    .map_err(database_error)?;
    let Some((name, reference, geometry)) = row else {
        return Err(ApiError::not_found("no such route"));
    };
    let title = name.unwrap_or_else(|| {
        reference.map_or_else(|| format!("Route {osm_id}"), |r| format!("CAI {r}"))
    });
    let bytes = gpx_bytes(&title, osm_id, &geometry).map_err(|error| {
        tracing::error!(%error, osm_id, "writing GPX failed");
        ApiError::internal()
    })?;
    Ok((
        [
            (CONTENT_TYPE, "application/gpx+xml".to_owned()),
            (
                CONTENT_DISPOSITION,
                format!("attachment; filename=\"aret-route-{osm_id}.gpx\""),
            ),
        ],
        bytes,
    ))
}

/// GPX 1.1 for a MultiLineString, with the OpenStreetMap credit.
fn gpx_bytes(title: &str, osm_id: i64, geometry: &Value) -> Result<Vec<u8>, String> {
    use gpx::{Gpx, GpxVersion, Link, Metadata, Track, TrackSegment, Waypoint};

    let lines = geometry["coordinates"]
        .as_array()
        .ok_or("geometry without coordinates")?;
    let mut track = Track::new();
    track.name = Some(title.to_owned());
    for line in lines {
        let mut segment = TrackSegment::new();
        for coordinate in line.as_array().ok_or("line without points")? {
            let (Some(lon), Some(lat)) = (coordinate[0].as_f64(), coordinate[1].as_f64()) else {
                return Err("point without longitude/latitude".to_owned());
            };
            segment
                .points
                .push(Waypoint::new(geo_types::Point::new(lon, lat)));
        }
        track.segments.push(segment);
    }
    let osm_url = format!("https://www.openstreetmap.org/relation/{osm_id}");
    let file = Gpx {
        version: GpxVersion::Gpx11,
        creator: Some("Aret".to_owned()),
        metadata: Some(Metadata {
            name: Some(title.to_owned()),
            description: Some(format!(
                "{ATTRIBUTION}, {LICENSE} (https://www.openstreetmap.org/copyright). \
                 Route from OpenStreetMap relation {osm_id}."
            )),
            links: vec![Link {
                href: osm_url,
                text: Some("OpenStreetMap".to_owned()),
                ..Link::default()
            }],
            ..Metadata::default()
        }),
        tracks: vec![track],
        ..Gpx::default()
    };
    let mut bytes = Vec::new();
    gpx::write(&file, &mut bytes).map_err(|error| error.to_string())?;
    Ok(bytes)
}
