//! Track statistics computed on the server. Distance comes from PostGIS (see [`crate::store`]).

use crate::model::TrackPoint;

/// Elevation changes smaller than this are treated as GPS/barometer noise, in metres.
pub const ELEVATION_NOISE_M: f64 = 3.0;

/// Total climb and descent of a track.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ElevationGain {
    /// Sum of climbs, in metres.
    pub ascent_m: f64,
    /// Sum of descents, in metres.
    pub descent_m: f64,
}

/// Sums climbs and descents, ignoring wiggles smaller than [`ELEVATION_NOISE_M`]: a change only
/// counts once the elevation has moved at least that far from the last counted point.
pub fn elevation_gain(points: &[TrackPoint]) -> ElevationGain {
    let Some(first) = points.first() else {
        return ElevationGain::default();
    };
    let mut gain = ElevationGain::default();
    let mut reference: f64 = first.ele;
    for point in &points[1..] {
        let change: f64 = point.ele - reference;
        if change >= ELEVATION_NOISE_M {
            gain.ascent_m += change;
            reference = point.ele;
        } else if change <= -ELEVATION_NOISE_M {
            gain.descent_m -= change;
            reference = point.ele;
        }
    }
    gain
}

#[cfg(test)]
mod tests {
    use super::*;

    use time::macros::datetime;

    fn track(elevations: &[f64]) -> Vec<TrackPoint> {
        elevations
            .iter()
            .map(|&ele| TrackPoint {
                lat: 44.0,
                lon: 10.0,
                ele,
                time: datetime!(2026-08-01 8:00 UTC),
            })
            .collect()
    }

    #[test]
    fn ignores_noise_below_the_threshold() {
        let gain = elevation_gain(&track(&[1000.0, 1002.0, 1001.0, 1002.5, 1000.5]));
        assert_eq!(gain, ElevationGain::default());
    }

    #[test]
    fn counts_climbs_and_descents() {
        let gain = elevation_gain(&track(&[1000.0, 1002.0, 1006.0, 1004.0, 1010.0, 990.0]));
        assert_eq!(
            gain,
            ElevationGain {
                ascent_m: 10.0,
                descent_m: 20.0
            }
        );
    }

    #[test]
    fn handles_an_empty_track() {
        assert_eq!(elevation_gain(&[]), ElevationGain::default());
    }
}
