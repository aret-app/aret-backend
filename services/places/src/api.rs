//! Public search API (no login: OpenStreetMap reference data, ODbL). Every answer carries the
//! credit, the licence and the date of the data.
//!
//! - `GET /v1/places?q=&limit=`: places by name, typo- and accent-tolerant, peaks favoured
//! - `GET /v1/routes?near=<place id>` or `?lat=&lon=` (`&radius_m=`): routes that reach the point
//!   (within 200 m) first, then routes nearby
//! - `GET /v1/routes/{osm_id}`: one route with geometry and elevation profile
//! - `GET /v1/routes/{osm_id}/gpx`: the route as GPX 1.1
//! - `GET /v1/tours?to=<place id>`: ready-to-walk round trips to a place ([`crate::tours`])

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

/// Query of `GET /v1/tours`.
#[derive(Debug, Deserialize)]
pub struct ToursQuery {
    /// The place to reach, e.g. `node/3140315349`.
    pub to: String,
    /// At most this many tours, 1–10 (default 6).
    pub limit: Option<usize>,
}

/// `GET /v1/tours?to=`: round trips from a car park or village to the place and back, through the
/// network of marked trails, with totals ([`crate::tours`]).
pub async fn tours_to(
    State(pool): State<PgPool>,
    State(cache): State<std::sync::Arc<TourCache>>,
    Query(query): Query<ToursQuery>,
) -> Result<Json<Value>, ApiError> {
    use crate::tours::{AccessKind, Shape};

    let data = data_info(&pool).await?;
    let place: Option<PlaceTarget> =
        sqlx::query_as("SELECT ST_X(geom), ST_Y(geom), name, kind, ele FROM places WHERE id = $1")
            .bind(&query.to)
            .fetch_optional(&pool)
            .await
            .map_err(database_error)?;
    let Some((lon, lat, name, kind, ele)) = place else {
        return Err(ApiError::not_found("no such place"));
    };
    let index = cache.get(&pool).await?;
    let limit = query.limit.unwrap_or(6).clamp(1, 10);
    let place_name = name.clone().unwrap_or_else(|| "the place".to_owned());
    let tours: Vec<Value> = crate::tours::tours(&index.network, &index.starts, (lon, lat), REACH_M, limit)
        .into_iter()
        .map(|tour| {
            let mut lines = vec![rounded(&tour.up.line)];
            let mut route_ids = tour.up.route_ids.clone();
            // The way down: the loop's other way (walked backwards), or the way up again
            let via_down: Vec<String> = match &tour.down {
                Some(down) => {
                    lines.push(rounded(&down.line.iter().rev().copied().collect::<Vec<_>>()));
                    route_ids.extend(down.route_ids.iter().rev());
                    down.via.iter().rev().cloned().collect()
                }
                None => tour.up.via.iter().rev().cloned().collect(),
            };
            json!({
                "shape": match tour.shape { Shape::OutAndBack => "out_and_back", Shape::Loop => "loop" },
                "title": format!("{place_name} from {}", tour.start.name),
                "start": {
                    "id": tour.start.id,
                    "name": tour.start.name,
                    "kind": match tour.start.kind { AccessKind::CarPark => "car_park", AccessKind::Village => "village" },
                    "position": [tour.start.position.0, tour.start.position.1],
                    "ele": tour.up.start_ele.round(),
                },
                "via_up": tour.up.via,
                "via_down": via_down,
                "route_ids": route_ids,
                "length_m": tour.length_m.round(),
                "ascent_m": tour.ascent_m.round(),
                "descent_m": tour.descent_m.round(),
                "duration_min": tour.duration_min,
                "difficulty": tour.difficulty,
                "ele_max": tour.max_ele.round(),
                "geometry": { "type": "MultiLineString", "coordinates": lines },
            })
        })
        .collect();
    Ok(Json(json!({
        "place": { "id": query.to, "name": name, "kind": kind, "ele": ele, "position": [lon, lat] },
        "tours": tours,
        "data": data,
    })))
}

/// longitude, latitude, name, kind, elevation of the place a tour goes to.
type PlaceTarget = (f64, f64, Option<String>, String, Option<f64>);

fn rounded(line: &[(f64, f64)]) -> Vec<[f64; 2]> {
    let round = |v: f64| (v * 1e6).round() / 1e6;
    line.iter()
        .map(|&(lon, lat)| [round(lon), round(lat)])
        .collect()
}

/// The trail network and the tour starts for one catalogue version, built on first use.
pub struct TourIndex {
    version: String,
    /// Marked trails joined where they meet.
    pub network: crate::network::Network,
    /// Car parks and villages attached to the network.
    pub starts: Vec<crate::tours::Access>,
}

/// Keeps the [`TourIndex`] of the loaded catalogue; rebuilt when a new catalogue is loaded.
#[derive(Default)]
pub struct TourCache {
    index: tokio::sync::Mutex<Option<std::sync::Arc<TourIndex>>>,
}

/// Car parks attach to a junction or route end within this distance; villages within the second.
const CAR_PARK_REACH_M: f64 = 300.0;
const VILLAGE_REACH_M: f64 = 800.0;

impl TourCache {
    /// Keeps the network built for the loaded catalogue: checks every `interval` (cheap when it's
    /// current), so the ~10 s build after a new catalogue never falls on a user's request.
    pub async fn keep_warm(&self, pool: PgPool, interval: std::time::Duration) {
        loop {
            let loaded: Option<bool> = sqlx::query_scalar("SELECT true FROM catalogue")
                .fetch_optional(&pool)
                .await
                .ok()
                .flatten();
            if loaded.is_some() && self.get(&pool).await.is_err() {
                tracing::warn!("building the trail network failed; retrying later");
            }
            tokio::time::sleep(interval).await;
        }
    }

    async fn get(&self, pool: &PgPool) -> Result<std::sync::Arc<TourIndex>, ApiError> {
        let version: String =
            sqlx::query_scalar("SELECT routes_sha256 || places_sha256 FROM catalogue")
                .fetch_one(pool)
                .await
                .map_err(database_error)?;
        let mut slot = self.index.lock().await;
        if let Some(index) = slot.as_ref().filter(|index| index.version == version) {
            return Ok(std::sync::Arc::clone(index));
        }
        let index = std::sync::Arc::new(build_index(pool, version).await.map_err(database_error)?);
        *slot = Some(std::sync::Arc::clone(&index));
        Ok(index)
    }
}

/// Route lines and profiles, car parks and villages from the database, as a [`TourIndex`].
async fn build_index(pool: &PgPool, version: String) -> Result<TourIndex, sqlx::Error> {
    use crate::network::{Network, RouteInput};
    use crate::tours::{Access, AccessKind};

    let fetched = std::time::Instant::now();
    let rows: Vec<RouteRow> = sqlx::query_as(
        "SELECT osm_id, name, ref, difficulty, ST_AsGeoJSON(geom, 7)::jsonb, profiles FROM routes \
         WHERE profiles IS NOT NULL",
    )
    .fetch_all(pool)
    .await?;
    let places: Vec<(String, Option<String>, String, f64, f64)> = sqlx::query_as(
        "SELECT id, name, kind, ST_X(geom), ST_Y(geom) FROM places \
         WHERE kind = 'trailhead' OR (kind IN ('settlement', 'peak', 'pass', 'hut') AND searchable)",
    )
    .fetch_all(pool)
    .await?;

    tracing::info!(
        seconds = fetched.elapsed().as_secs_f64(),
        "routes and places read for the network"
    );
    let loaded = std::time::Instant::now();
    let index = tokio::task::spawn_blocking(move || {
        let routes: Vec<RouteInput> = rows
            .into_iter()
            .filter_map(
                |(osm_id, name, reference, difficulty, geometry, profiles)| {
                    let lines: Vec<Vec<(f64, f64)>> = geometry["coordinates"]
                        .as_array()?
                        .iter()
                        .map(pairs)
                        .collect::<Option<_>>()?;
                    let profiles: Vec<Vec<(f64, f64)>> = profiles?
                        .as_array()?
                        .iter()
                        .map(pairs)
                        .collect::<Option<_>>()?;
                    let label = match (&reference, &name) {
                        (Some(r), _) => format!("CAI {r}"),
                        (None, Some(n)) => n.clone(),
                        (None, None) => format!("route {osm_id}"),
                    };
                    Some(RouteInput {
                        osm_id,
                        label,
                        difficulty,
                        lines,
                        profiles,
                    })
                },
            )
            .collect();
        let built = std::time::Instant::now();
        let network = Network::build(&routes);
        tracing::info!(
            seconds = built.elapsed().as_secs_f64(),
            "network graph built"
        );

        // Names for unnamed car parks: the closest village (within 3 km) or named peak, pass or
        // hut (within 800 m), e.g. "car park near Piglionico"
        let landmarks: Vec<(&str, f64, (f64, f64))> = places
            .iter()
            .filter(|(_, name, kind, _, _)| kind != "trailhead" && name.is_some())
            .map(|(_, name, kind, lon, lat)| {
                let within = if kind == "settlement" { 3_000.0 } else { 800.0 };
                (name.as_deref().unwrap_or_default(), within, (*lon, *lat))
            })
            .collect();
        // A quick flat-earth distance is plenty to pick a name (thousands of car parks × places)
        let approx_m = |a: (f64, f64), b: (f64, f64)| {
            let dx = (a.0 - b.0) * 111_320.0 * a.1.to_radians().cos();
            let dy = (a.1 - b.1) * 110_540.0;
            (dx * dx + dy * dy).sqrt()
        };
        let near_village = |point: (f64, f64)| {
            landmarks
                .iter()
                .filter(|&&(_, _, at)| {
                    (at.0 - point.0).abs() < 0.05 && (at.1 - point.1).abs() < 0.03
                })
                .map(|&(name, within, at)| (name, within, approx_m(point, at)))
                .filter(|&(_, within, d)| d <= within)
                .min_by(|a, b| a.2.total_cmp(&b.2))
                .map(|(name, _, _)| name.to_owned())
        };
        let starts: Vec<Access> = places
            .iter()
            .filter(|(_, _, kind, _, _)| kind == "trailhead" || kind == "settlement")
            .filter_map(|(id, name, kind, lon, lat)| {
                let position = (*lon, *lat);
                let (access_kind, reach) = if kind == "trailhead" {
                    (AccessKind::CarPark, CAR_PARK_REACH_M)
                } else {
                    (AccessKind::Village, VILLAGE_REACH_M)
                };
                let (node, _) = network.nearest_node(position, reach)?;
                let name = match (name, access_kind) {
                    (Some(name), _) => name.clone(),
                    (None, AccessKind::CarPark) => near_village(position)
                        .map_or_else(|| "a car park".to_owned(), |v| format!("car park near {v}")),
                    (None, AccessKind::Village) => return None,
                };
                Some(Access {
                    id: id.clone(),
                    name,
                    kind: access_kind,
                    position,
                    node,
                })
            })
            .collect();
        TourIndex {
            version,
            network,
            starts,
        }
    })
    .await
    .map_err(|error| sqlx::Error::Protocol(format!("building the trail network: {error}")))?;
    tracing::info!(
        nodes = index.network.nodes.len(),
        edges = index.network.edges.len(),
        starts = index.starts.len(),
        seconds = loaded.elapsed().as_secs_f64(),
        "trail network built"
    );
    Ok(index)
}

/// osm_id, name, ref, difficulty, geometry (GeoJSON), profiles.
type RouteRow = (
    i64,
    Option<String>,
    Option<String>,
    String,
    Value,
    Option<Value>,
);

/// `[[a, b], …]` as pairs, or `None` if malformed.
fn pairs(value: &Value) -> Option<Vec<(f64, f64)>> {
    value
        .as_array()?
        .iter()
        .map(|pair| Some((pair.get(0)?.as_f64()?, pair.get(1)?.as_f64()?)))
        .collect()
}
