//! Everything a tour's detail card needs (aret-backend#24): the round-trip track, an elevation
//! profile with the position of each sample (so hovering the chart can mark the spot on the map),
//! highest and lowest points, the long climbs and descents, the named places along the way, and a
//! plain-language description. The front end draws; it computes nothing.

use std::collections::HashMap;

use crate::network::distance;
use crate::tours::{AccessKind, Shape, Tour};

/// Distance between profile samples, in metres.
pub const SAMPLE_STEP_M: f64 = 50.0;
/// A named place counts as passed if the track comes this close.
pub const WAYPOINT_M: f64 = 75.0;
/// A climb or descent is listed from this change in elevation.
const MIN_SLOPE_M: f64 = 100.0;
/// A slope ends when the elevation turns back by more than this.
const TURN_M: f64 = 30.0;
/// Gradients are measured over this length (two samples), so one odd sample doesn't decide.
const GRADIENT_SPAN_M: f64 = 100.0;

/// The whole round trip, start → place → start.
#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    /// (longitude, latitude) points.
    pub line: Vec<(f64, f64)>,
    /// (distance from the start in metres, elevation in metres).
    pub profile: Vec<(f64, f64)>,
}

/// One profile sample.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    /// Distance from the start, in metres.
    pub distance_m: f64,
    /// Elevation in metres.
    pub ele: f64,
    /// (longitude, latitude).
    pub position: (f64, f64),
}

/// A long climb or descent.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slope {
    /// Where it starts and ends, in metres from the start.
    pub from_m: f64,
    /// Where it ends.
    pub to_m: f64,
    /// Elevation gained (climb) or lost (descent), in metres.
    pub change_m: f64,
    /// Average gradient, percent.
    pub gradient_pct: f64,
    /// Steepest gradient over 100 m, percent.
    pub max_gradient_pct: f64,
}

/// A named place the tour passes.
#[derive(Debug, Clone, PartialEq)]
pub struct Landmark {
    /// Places id.
    pub id: String,
    /// Name.
    pub name: String,
    /// peak, pass, hut, water, settlement.
    pub kind: String,
    /// Elevation, if known.
    pub ele: Option<f64>,
    /// (longitude, latitude).
    pub position: (f64, f64),
}

/// Landmarks by grid cell (~1 km) for quick lookups along a track.
#[derive(Debug, Default)]
pub struct Landmarks {
    all: Vec<Landmark>,
    grid: HashMap<(i64, i64), Vec<usize>>,
}

impl Landmarks {
    /// Indexes `all`.
    pub fn new(all: Vec<Landmark>) -> Self {
        let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
        for (index, landmark) in all.iter().enumerate() {
            grid.entry(cell(landmark.position)).or_default().push(index);
        }
        Self { all, grid }
    }

    fn near(&self, point: (f64, f64)) -> impl Iterator<Item = (usize, &Landmark)> {
        let (x, y) = cell(point);
        (x - 1..=x + 1)
            .flat_map(move |cx| (y - 1..=y + 1).map(move |cy| (cx, cy)))
            .filter_map(|c| self.grid.get(&c))
            .flatten()
            .map(|&index| (index, &self.all[index]))
    }
}

fn cell((lon, lat): (f64, f64)) -> (i64, i64) {
    ((lon / 0.01).floor() as i64, (lat / 0.01).floor() as i64)
}

/// A landmark the tour passes, where.
#[derive(Debug, Clone, PartialEq)]
pub struct Waypoint {
    /// The place.
    pub landmark: Landmark,
    /// Distance from the start where the track passes closest, in metres.
    pub distance_m: f64,
}

/// The round trip: up, then back the same way (out-and-back) or down the other way (loop).
pub fn track(tour: &Tour) -> Track {
    let reversed = |line: &[(f64, f64)], profile: &[(f64, f64)]| {
        let length = profile.last().map_or(0.0, |&(d, _)| d);
        (
            line.iter().rev().copied().collect::<Vec<_>>(),
            profile
                .iter()
                .rev()
                .map(|&(d, e)| (length - d, e))
                .collect::<Vec<_>>(),
        )
    };
    let (down_line, down_profile) = match &tour.down {
        Some(down) => reversed(&down.line, &down.profile),
        None => reversed(&tour.up.line, &tour.up.profile),
    };
    let mut line = tour.up.line.clone();
    line.extend(down_line.into_iter().skip(1));
    let offset = tour.up.profile.last().map_or(0.0, |&(d, _)| d);
    let mut profile = tour.up.profile.clone();
    profile.extend(
        down_profile
            .into_iter()
            .skip(1)
            .map(|(d, e)| (offset + d, e)),
    );
    Track { line, profile }
}

/// Samples every [`SAMPLE_STEP_M`] along the track, with their positions (and the last point).
pub fn samples(track: &Track) -> Vec<Sample> {
    let along = cumulative(&track.line);
    let Some(&line_length) = along.last() else {
        return Vec::new();
    };
    let profile_length = track.profile.last().map_or(0.0, |&(d, _)| d);
    // The profile's and the line's lengths differ by a fraction of a percent (measured slightly
    // differently); scale so both end together
    let scale = if line_length > 0.0 {
        profile_length / line_length
    } else {
        1.0
    };
    let mut out = Vec::new();
    let mut distance_m: f64 = 0.0;
    loop {
        let at = distance_m.min(line_length);
        if let (Some(position), Some(ele)) = (
            position_at(&track.line, &along, at),
            elevation_at(&track.profile, at * scale),
        ) {
            out.push(Sample {
                distance_m: at,
                ele,
                position,
            });
        }
        if at >= line_length {
            break;
        }
        distance_m += SAMPLE_STEP_M;
    }
    out
}

/// The elevation of every point of the track's line (for the GPX), if the profile has one.
pub fn elevations(track: &Track) -> Vec<Option<f64>> {
    let along = cumulative(&track.line);
    let line_length = along.last().copied().unwrap_or(0.0);
    let profile_length = track.profile.last().map_or(0.0, |&(d, _)| d);
    let scale = if line_length > 0.0 {
        profile_length / line_length
    } else {
        1.0
    };
    along
        .iter()
        .map(|&at| elevation_at(&track.profile, at * scale))
        .collect()
}

/// Highest and lowest sample.
pub fn extremes(samples: &[Sample]) -> Option<(Sample, Sample)> {
    let highest = samples
        .iter()
        .copied()
        .max_by(|a, b| a.ele.total_cmp(&b.ele))?;
    let lowest = samples
        .iter()
        .copied()
        .min_by(|a, b| a.ele.total_cmp(&b.ele))?;
    Some((highest, lowest))
}

/// The long climbs and descents (at least 100 m of change), in order along the track.
pub fn slopes(samples: &[Sample]) -> (Vec<Slope>, Vec<Slope>) {
    if samples.len() < 2 {
        return (Vec::new(), Vec::new());
    }
    // Turning points: where the elevation reverses by more than TURN_M from the running extreme
    let mut turns = vec![0];
    let mut extreme = 0;
    let mut rising: Option<bool> = None;
    for (i, sample) in samples.iter().enumerate().skip(1) {
        let change = sample.ele - samples[extreme].ele;
        match rising {
            None => {
                if change.abs() > TURN_M {
                    rising = Some(change > 0.0);
                    extreme = i;
                }
            }
            Some(up) => {
                if (up && change > 0.0) || (!up && change < 0.0) {
                    extreme = i;
                } else if change.abs() > TURN_M {
                    turns.push(extreme);
                    rising = Some(!up);
                    extreme = i;
                }
            }
        }
    }
    turns.push(extreme);
    turns.push(samples.len() - 1);
    turns.dedup();

    let (mut climbs, mut descents) = (Vec::new(), Vec::new());
    for pair in turns.windows(2) {
        let (a, b) = (samples[pair[0]], samples[pair[1]]);
        let change = b.ele - a.ele;
        let length = b.distance_m - a.distance_m;
        if change.abs() < MIN_SLOPE_M || length <= 0.0 {
            continue;
        }
        let span = (GRADIENT_SPAN_M / SAMPLE_STEP_M).round().max(1.0) as usize;
        let steepest = (pair[0]..pair[1].saturating_sub(span - 1).max(pair[0] + 1))
            .filter_map(|i| {
                let j = (i + span).min(pair[1]);
                let d = samples[j].distance_m - samples[i].distance_m;
                (d > 0.0).then(|| ((samples[j].ele - samples[i].ele) / d * 100.0).abs())
            })
            .fold(0.0, f64::max);
        let slope = Slope {
            from_m: a.distance_m,
            to_m: b.distance_m,
            change_m: change.abs(),
            gradient_pct: (change.abs() / length * 100.0),
            max_gradient_pct: steepest,
        };
        if change > 0.0 {
            climbs.push(slope);
        } else {
            descents.push(slope);
        }
    }
    (climbs, descents)
}

/// The named places within [`WAYPOINT_M`] of the track, each once (where first passed), in order.
pub fn waypoints(samples: &[Sample], landmarks: &Landmarks) -> Vec<Waypoint> {
    let mut best: HashMap<usize, (f64, f64)> = HashMap::new(); // landmark → (offset, distance along)
    for sample in samples {
        for (index, landmark) in landmarks.near(sample.position) {
            let offset = distance(sample.position, landmark.position);
            if offset > WAYPOINT_M {
                continue;
            }
            // First pass only: on the way back the same places are passed again
            let entry = best.entry(index).or_insert((offset, sample.distance_m));
            let first_pass = sample.distance_m - entry.1 < 2.0 * WAYPOINT_M;
            if first_pass && offset < entry.0 {
                *entry = (offset, sample.distance_m);
            }
        }
    }
    let mut found: Vec<Waypoint> = best
        .into_iter()
        .map(|(index, (_, distance_m))| Waypoint {
            landmark: landmarks.all[index].clone(),
            distance_m,
        })
        .collect();
    found.sort_by(|a, b| a.distance_m.total_cmp(&b.distance_m));
    found
}

/// The CAI grade's name and what it means for a hiker.
pub fn difficulty_info(grade: &str) -> (&'static str, &'static str) {
    match grade {
        "T" => (
            "Tourist (T)",
            "Easy walk on clear, wide paths without steep or exposed stretches.",
        ),
        "E" => (
            "Hiking (E)",
            "Marked mountain paths, sometimes steep or on rough ground: a sure foot and hiking boots needed.",
        ),
        "EE" => (
            "Expert hikers (EE)",
            "Steep, exposed or trackless stretches, scree or easy rock: sure-footedness, a head for heights and mountain experience needed.",
        ),
        g if g.starts_with("EEA") => (
            "Equipped route (EEA)",
            "Via ferrata or protected passages: climbing harness, via ferrata kit and experience needed.",
        ),
        _ => (
            "Unknown",
            "Difficulty not recorded in OpenStreetMap for every stretch: check the trail before you go.",
        ),
    }
}

/// How fit a hiker should be, from walking time and climbing.
pub fn fitness(duration_min: u32, ascent_m: f64) -> &'static str {
    if duration_min <= 180 && ascent_m <= 600.0 {
        "Moderate fitness is enough."
    } else if duration_min <= 360 && ascent_m <= 1200.0 {
        "Good fitness needed."
    } else {
        "Very good fitness needed: a long, demanding day."
    }
}

/// A short plain-language description from the tour's facts. Never says a tour is safe: times are
/// estimates and conditions change.
pub fn description(tour: &Tour, place_name: &str) -> String {
    let (_, meaning) = difficulty_info(&tour.difficulty);
    let shape = match tour.shape {
        Shape::Loop => "A loop: up one way, down another.",
        Shape::OutAndBack => "Out and back on the same path.",
    };
    let start = match tour.start.kind {
        AccessKind::CarPark => format!("It starts at a car park ({}).", tour.start.name),
        AccessKind::Village => format!("It starts in {}.", tour.start.name),
    };
    format!(
        "{place_name} from {}. {meaning} {} {shape} {start} The walking time is an estimate without breaks; check the weather and the trail before you go.",
        tour.start.name,
        fitness(tour.duration_min, tour.ascent_m),
    )
}

fn cumulative(line: &[(f64, f64)]) -> Vec<f64> {
    let mut total = 0.0;
    let mut out = vec![0.0];
    for pair in line.windows(2) {
        total += distance(pair[0], pair[1]);
        out.push(total);
    }
    out
}

fn position_at(line: &[(f64, f64)], along: &[f64], at: f64) -> Option<(f64, f64)> {
    let after = along.iter().position(|&d| d >= at)?;
    if after == 0 {
        return line.first().copied();
    }
    let (d0, d1) = (along[after - 1], along[after]);
    let t = if d1 > d0 { (at - d0) / (d1 - d0) } else { 0.0 };
    let (a, b) = (line[after - 1], line[after]);
    Some((a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t))
}

fn elevation_at(profile: &[(f64, f64)], at: f64) -> Option<f64> {
    let last = profile.last()?;
    if at >= last.0 {
        return Some(last.1);
    }
    let after = profile.iter().position(|&(d, _)| d >= at)?;
    if after == 0 {
        return Some(profile[0].1);
    }
    let ((d0, e0), (d1, e1)) = (profile[after - 1], profile[after]);
    let t = if d1 > d0 { (at - d0) / (d1 - d0) } else { 0.0 };
    Some(e0 + (e1 - e0) * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Samples every 50 m with the given elevations, along a line north from (10.3, 44.0).
    fn profile(elevations: &[f64]) -> Vec<Sample> {
        elevations
            .iter()
            .enumerate()
            .map(|(i, &ele)| Sample {
                distance_m: 50.0 * i as f64,
                ele,
                position: (10.3, 44.0 + 0.00045 * i as f64),
            })
            .collect()
    }

    #[test]
    fn samples_carry_their_position() {
        // ~1 km north, climbing 200 m
        let track = Track {
            line: vec![(10.3, 44.0), (10.3, 44.009)],
            profile: vec![(0.0, 500.0), (1000.0, 700.0)],
        };
        let found = samples(&track);
        assert_eq!(found.len(), 22, "0, 50, …, 1000 m and the end");
        let middle = found[10];
        assert!((middle.distance_m - 500.0).abs() < 1e-9);
        assert!(
            (middle.position.1 - 44.0045).abs() < 0.0001,
            "{:?}",
            middle.position
        );
        assert!((middle.ele - 600.0).abs() < 2.0, "{}", middle.ele);
    }

    #[test]
    fn finds_long_climbs_and_descents_but_not_bumps() {
        // Up 300 m, a 20 m dip, up 200 m more, then down 500 m
        let mut elevations: Vec<f64> = (0..=15).map(|i| 1000.0 + 20.0 * f64::from(i)).collect();
        elevations.extend([1290.0, 1280.0]);
        elevations.extend((1..=10).map(|i| 1280.0 + 22.0 * f64::from(i)));
        elevations.extend((1..=25).map(|i| 1500.0 - 20.0 * f64::from(i)));
        let (climbs, descents) = slopes(&profile(&elevations));
        assert_eq!(
            climbs.len(),
            1,
            "the 20 m dip doesn't split the climb: {climbs:?}"
        );
        assert!(
            (climbs[0].change_m - 500.0).abs() < 1.0,
            "{}",
            climbs[0].change_m
        );
        assert_eq!(descents.len(), 1);
        assert!((descents[0].change_m - 500.0).abs() < 1.0);
        assert!(descents[0].gradient_pct > 39.0 && descents[0].gradient_pct < 41.0);
    }

    #[test]
    fn lists_places_passed_once_in_order() {
        let samples = profile(&[500.0; 30]);
        let landmarks = Landmarks::new(vec![
            Landmark {
                id: "node/2".into(),
                name: "Passo Alto".into(),
                kind: "pass".into(),
                ele: Some(600.0),
                position: (10.3003, 44.0090), // ~25 m east of the track at 1 km
            },
            Landmark {
                id: "node/1".into(),
                name: "Rifugio Basso".into(),
                kind: "hut".into(),
                ele: Some(550.0),
                position: (10.3, 44.0020),
            },
            Landmark {
                id: "node/3".into(),
                name: "Far Away".into(),
                kind: "peak".into(),
                ele: None,
                position: (10.31, 44.005),
            },
        ]);
        let found = waypoints(&samples, &landmarks);
        let names: Vec<&str> = found.iter().map(|w| w.landmark.name.as_str()).collect();
        assert_eq!(names, vec!["Rifugio Basso", "Passo Alto"]);
        assert!((found[1].distance_m - 1000.0).abs() < 60.0);
    }

    #[test]
    fn grades_are_explained_and_unknown_says_check() {
        assert_eq!(difficulty_info("EE").0, "Expert hikers (EE)");
        assert!(difficulty_info("unknown").1.contains("check"));
        assert_eq!(fitness(150, 500.0), "Moderate fitness is enough.");
        assert!(fitness(500, 1500.0).starts_with("Very good"));
    }
}
