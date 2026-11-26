//! GPX 1.1 import and export (aret-backend#17), the standard file format for hiking tracks.

use aret_common::error::ApiError;
use geo_types::Point;
use gpx::{Gpx, GpxVersion, Track, TrackSegment, Waypoint};
use time::OffsetDateTime;

use crate::model::{TrackDetail, TrackPoint};

/// A recorded track read from a GPX file.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedTrack {
    /// Track name, else the file's metadata name, else empty.
    pub title: String,
    /// Points of the first track, all segments joined in order.
    pub points: Vec<TrackPoint>,
}

/// Reads the first track (`<trk>`) of a GPX file.
///
/// Every point needs a time (a recorded track, not a planned route). Missing elevations are filled
/// in from the neighbouring points; a file without any elevation is rejected.
pub fn parse_track(data: &[u8]) -> Result<ImportedTrack, ApiError> {
    let file: Gpx = gpx::read(data).map_err(|_| ApiError::bad_request("not a valid GPX file"))?;
    let Some(track) = file.tracks.first() else {
        return Err(ApiError::bad_request("the GPX file has no track (<trk>)"));
    };
    let title: String = track
        .name
        .clone()
        .or_else(|| {
            file.metadata
                .as_ref()
                .and_then(|metadata| metadata.name.clone())
        })
        .unwrap_or_default();

    let waypoints: Vec<&Waypoint> = track
        .segments
        .iter()
        .flat_map(|segment| &segment.points)
        .collect();
    let elevations: Vec<Option<f64>> = waypoints.iter().map(|point| point.elevation).collect();
    let Some(elevations) = fill_missing_elevations(&elevations) else {
        return Err(ApiError::bad_request("the GPX track has no elevation data"));
    };

    let mut points: Vec<TrackPoint> = Vec::with_capacity(waypoints.len());
    for (index, (waypoint, ele)) in waypoints.iter().zip(elevations).enumerate() {
        let Some(time) = waypoint.time else {
            return Err(ApiError::bad_request(&format!(
                "point {index} has no time: only recorded tracks can be imported"
            )));
        };
        let position: Point<f64> = waypoint.point();
        points.push(TrackPoint {
            lat: position.y(),
            lon: position.x(),
            ele,
            time: OffsetDateTime::from(time),
        });
    }
    Ok(ImportedTrack { title, points })
}

/// Writes a track as a GPX 1.1 file with one segment.
pub fn write_track(track: &TrackDetail) -> Result<Vec<u8>, gpx::errors::GpxError> {
    let mut segment = TrackSegment::new();
    segment.points = track
        .points
        .iter()
        .map(|point| {
            let mut waypoint = Waypoint::new(Point::new(point.lon, point.lat));
            waypoint.elevation = Some(point.ele);
            waypoint.time = Some(point.time.into());
            waypoint
        })
        .collect();

    let mut gpx_track = Track::new();
    if !track.summary.title.is_empty() {
        gpx_track.name = Some(track.summary.title.clone());
    }
    gpx_track.segments.push(segment);

    let file = Gpx {
        version: GpxVersion::Gpx11,
        creator: Some("Aret".to_owned()),
        tracks: vec![gpx_track],
        ..Gpx::default()
    };
    let mut bytes: Vec<u8> = Vec::new();
    gpx::write(&file, &mut bytes)?;
    Ok(bytes)
}

/// Fills gaps by linear interpolation between the nearest known elevations (by point index);
/// gaps at the ends take the nearest known value. `None` if no elevation is known at all.
fn fill_missing_elevations(elevations: &[Option<f64>]) -> Option<Vec<f64>> {
    let known: Vec<(usize, f64)> = elevations
        .iter()
        .enumerate()
        .filter_map(|(index, ele)| ele.map(|value| (index, value)))
        .collect();
    let (first, last) = (*known.first()?, *known.last()?);

    let mut filled: Vec<f64> = Vec::with_capacity(elevations.len());
    let mut next_known = 0;
    for index in 0..elevations.len() {
        while next_known < known.len() && known[next_known].0 < index {
            next_known += 1;
        }
        let value = match (
            next_known.checked_sub(1).map(|i| known[i]),
            known.get(next_known),
        ) {
            (_, Some(&(at, ele))) if at == index => ele,
            (Some((before_at, before)), Some(&(after_at, after))) => {
                let share = (index - before_at) as f64 / (after_at - before_at) as f64;
                before + (after - before) * share
            }
            (None, _) => first.1,
            (_, None) => last.1,
        };
        filled.push(value);
    }
    Some(filled)
}

#[cfg(test)]
mod tests {
    use super::*;

    use time::macros::datetime;

    use crate::model::TrackSummary;

    const SAMPLE: &[u8] = include_bytes!("../tests/fixtures/sample.gpx");

    #[test]
    fn reads_all_segments_of_the_first_track() {
        let track = parse_track(SAMPLE).unwrap();
        assert_eq!(track.title, "Sample climb to the ridge");
        assert_eq!(track.points.len(), 6, "two segments joined");
        assert_eq!(track.points[0].lat, 44.03);
        assert_eq!(track.points[0].lon, 10.32);
        assert_eq!(track.points[5].time, datetime!(2026-08-02 07:22 UTC));
    }

    #[test]
    fn interpolates_a_missing_elevation() {
        let track = parse_track(SAMPLE).unwrap();
        assert_eq!(track.points[2].ele, 1124.0, "halfway between 1112 and 1136");
    }

    #[test]
    fn fills_the_ends_from_the_nearest_elevation() {
        let filled = fill_missing_elevations(&[None, Some(10.0), None, Some(20.0), None]).unwrap();
        assert_eq!(filled, vec![10.0, 10.0, 15.0, 20.0, 20.0]);
        assert_eq!(fill_missing_elevations(&[None, None]), None);
    }

    #[test]
    fn rejects_points_without_time() {
        let route = String::from_utf8(SAMPLE.to_vec())
            .unwrap()
            .replace("<time>2026-08-02T07:02:00Z</time>", "");
        let err = parse_track(route.as_bytes()).unwrap_err();
        assert_eq!(
            err.to_string(),
            "400 Bad Request: point 1 has no time: only recorded tracks can be imported"
        );
    }

    #[test]
    fn rejects_files_that_are_not_gpx() {
        assert!(parse_track(b"<html>nope</html>").is_err());
        assert!(parse_track(b"").is_err());
    }

    #[test]
    fn export_and_import_round_trip() {
        let imported = parse_track(SAMPLE).unwrap();
        let detail = TrackDetail {
            summary: TrackSummary {
                id: uuid::Uuid::from_u128(7),
                title: imported.title.clone(),
                started_at: imported.points[0].time,
                ended_at: imported.points[5].time,
                distance_m: 0.0,
                ascent_m: 0.0,
                descent_m: 0.0,
                point_count: 6,
            },
            points: imported.points.clone(),
        };
        let exported = write_track(&detail).unwrap();
        assert_eq!(parse_track(&exported).unwrap(), imported);
    }
}
