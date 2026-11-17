//! SQL for tracks. Every query is scoped to the owner: one user can never see another's tracks.

use std::fmt::Write as _;

use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::model::{PageCursor, TrackPoint, TrackSummary, ValidTrack};
use crate::stats::ElevationGain;

/// Result of saving a track.
#[derive(Debug, Clone, PartialEq)]
pub enum SaveOutcome {
    /// New track stored.
    Created(TrackSummary),
    /// The same user already saved this id (a retried upload): nothing changed.
    AlreadySaved(TrackSummary),
    /// Another user's track has this id.
    IdTakenByAnotherUser,
}

/// Stores a validated track for `owner`; idempotent on the track id. The distance is computed by
/// PostGIS on the WGS 84 ellipsoid.
pub async fn save_track(
    pool: &PgPool,
    owner: Uuid,
    track: &ValidTrack,
    gain: ElevationGain,
) -> Result<SaveOutcome, sqlx::Error> {
    let point_count =
        i32::try_from(track.points.len()).expect("point count is validated to fit in i32");
    let created: Option<TrackSummary> = sqlx::query_as(
        "INSERT INTO tracks (id, owner_id, title, started_at, ended_at, distance_m, ascent_m, \
                             descent_m, point_count, geom) \
         SELECT $1, $2, $3, $4, $5, ST_Length(ST_Force2D(line.geom)::geography), $6, $7, $8, \
                line.geom \
         FROM (SELECT ST_GeomFromText($9, 4326) AS geom) AS line \
         ON CONFLICT (id) DO NOTHING \
         RETURNING id, title, started_at, ended_at, distance_m, ascent_m, descent_m, point_count",
    )
    .bind(track.id)
    .bind(owner)
    .bind(&track.title)
    .bind(track.started_at())
    .bind(track.ended_at())
    .bind(gain.ascent_m)
    .bind(gain.descent_m)
    .bind(point_count)
    .bind(line_string_zm(&track.points))
    .fetch_optional(pool)
    .await?;

    if let Some(summary) = created {
        return Ok(SaveOutcome::Created(summary));
    }
    match get_summary(pool, owner, track.id).await? {
        Some(summary) => Ok(SaveOutcome::AlreadySaved(summary)),
        None => Ok(SaveOutcome::IdTakenByAnotherUser),
    }
}

/// One track of `owner`, without points.
pub async fn get_summary(
    pool: &PgPool,
    owner: Uuid,
    id: Uuid,
) -> Result<Option<TrackSummary>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, title, started_at, ended_at, distance_m, ascent_m, descent_m, point_count \
         FROM tracks WHERE id = $1 AND owner_id = $2",
    )
    .bind(id)
    .bind(owner)
    .fetch_optional(pool)
    .await
}

/// All points of one track of `owner`, in recording order.
pub async fn get_points(
    pool: &PgPool,
    owner: Uuid,
    id: Uuid,
) -> Result<Vec<TrackPoint>, sqlx::Error> {
    let rows: Vec<PointRow> = sqlx::query_as(
        "SELECT ST_Y(dump.geom) AS lat, ST_X(dump.geom) AS lon, ST_Z(dump.geom) AS ele, \
                ST_M(dump.geom) AS unix_seconds \
         FROM tracks CROSS JOIN LATERAL ST_DumpPoints(tracks.geom) AS dump \
         WHERE tracks.id = $1 AND tracks.owner_id = $2 \
         ORDER BY dump.path",
    )
    .bind(id)
    .bind(owner)
    .fetch_all(pool)
    .await?;
    rows.into_iter().map(PointRow::try_into_point).collect()
}

/// Tracks of `owner`, newest first, starting after `after`; at most `limit` rows.
pub async fn list_summaries(
    pool: &PgPool,
    owner: Uuid,
    after: Option<&PageCursor>,
    limit: i64,
) -> Result<Vec<TrackSummary>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, title, started_at, ended_at, distance_m, ascent_m, descent_m, point_count \
         FROM tracks \
         WHERE owner_id = $1 AND ($2::timestamptz IS NULL OR (started_at, id) < ($2, $3)) \
         ORDER BY started_at DESC, id DESC \
         LIMIT $4",
    )
    .bind(owner)
    .bind(after.map(|cursor| cursor.started_at))
    .bind(after.map(|cursor| cursor.id))
    .bind(limit)
    .fetch_all(pool)
    .await
}

/// A point as stored: the time is the line's M value in Unix seconds.
#[derive(sqlx::FromRow)]
struct PointRow {
    lat: f64,
    lon: f64,
    ele: f64,
    unix_seconds: f64,
}

impl PointRow {
    fn try_into_point(self) -> Result<TrackPoint, sqlx::Error> {
        // Seconds with millisecond precision, as written by `line_string_zm`
        let nanos: i128 = (self.unix_seconds * 1_000.0).round() as i128 * 1_000_000;
        let time = OffsetDateTime::from_unix_timestamp_nanos(nanos)
            .map_err(|error| sqlx::Error::Decode(Box::new(error)))?;
        Ok(TrackPoint {
            lat: self.lat,
            lon: self.lon,
            ele: self.ele,
            time,
        })
    }
}

/// Well-known text for PostGIS: `LINESTRING ZM (lon lat ele unix_seconds, …)`.
fn line_string_zm(points: &[TrackPoint]) -> String {
    let mut wkt = String::with_capacity(points.len() * 56 + 16);
    wkt.push_str("LINESTRING ZM (");
    for (index, point) in points.iter().enumerate() {
        if index > 0 {
            wkt.push_str(", ");
        }
        let unix_seconds: f64 = point.time.unix_timestamp_nanos() as f64 / 1e9;
        write!(
            wkt,
            "{:.7} {:.7} {:.2} {:.3}",
            point.lon, point.lat, point.ele, unix_seconds
        )
        .expect("writing to a String cannot fail");
    }
    wkt.push(')');
    wkt
}

#[cfg(test)]
mod tests {
    use super::*;

    use time::macros::datetime;

    #[test]
    fn writes_longitude_first_with_elevation_and_time() {
        let points = [
            TrackPoint {
                lat: 44.0,
                lon: 10.25,
                ele: 1000.0,
                time: datetime!(2026-08-01 0:00 UTC),
            },
            TrackPoint {
                lat: 44.001,
                lon: 10.25,
                ele: 1002.5,
                time: datetime!(2026-08-01 0:00:01.5 UTC),
            },
        ];
        assert_eq!(
            line_string_zm(&points),
            "LINESTRING ZM (10.2500000 44.0000000 1000.00 1785542400.000, \
             10.2500000 44.0010000 1002.50 1785542401.500)"
        );
    }
}
