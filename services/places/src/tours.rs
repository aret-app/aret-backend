//! Tours to a place: ready-to-walk round trips from an accessible start (aret-backend#24).
//!
//! A hiker searching "Pania della Croce" wants a few complete outings, each from a car park or a
//! village to the top and back, with round-trip totals, not the whole trail network. Using the
//! [`Network`] of marked trails (joined only where routes really meet):
//!
//! - **Starts:** car parks and trailheads within 300 m of a trail, villages within 800 m.
//! - **Way up:** the least effort from the start through the network to the place.
//! - **Loop:** a second search that avoids the way up; if it shares at most 30% of it and takes at
//!   most 1.5 times as long, the tour goes up one way and down the other. Otherwise it's an
//!   out-and-back.
//! - **Totals:** distance, climbing and DIN 33466 time per direction (as the catalogue); difficulty
//!   is the hardest along the way, and "unknown" if any stretch's is unknown.
//! - Up to `limit` tours from starts at least 1 km apart, quickest first.

use crate::network::{self, Network, Part, Reach, Step};

/// (longitude, latitude) points, or (distance, elevation) samples.
type Points = Vec<(f64, f64)>;

/// Furthest one-way effort considered, in hours of the network's additive effort (distance time +
/// climbing time), which runs higher than DIN 33466's walking time: 9 h is about 6–6.5 h of
/// walking one way, enough for the long classics (Fornovolasco to Pania della Croce).
pub const MAX_ONE_WAY_H: f64 = 9.0;
/// Starts closer than this give the same tour.
const DISTINCT_STARTS_M: f64 = 1_000.0;
/// A way down may share at most this share of the way up to count as a loop.
const LOOP_MAX_OVERLAP: f64 = 0.3;
/// ... and take at most this many times the way up's effort.
const LOOP_MAX_LONGER: f64 = 1.5;
/// Extra cost of the way up's edges when looking for a different way down.
const LOOP_PENALTY: f64 = 4.0;

/// What kind of start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessKind {
    /// Car park or trailhead.
    CarPark,
    /// Village, town or hamlet.
    Village,
}

impl AccessKind {
    /// Car parks before villages at the same effort.
    fn rank(self) -> u8 {
        match self {
            Self::CarPark => 0,
            Self::Village => 1,
        }
    }
}

/// A place a tour can start from, attached to the network.
#[derive(Debug, Clone, PartialEq)]
pub struct Access {
    /// Places id, e.g. `way/123`.
    pub id: String,
    /// "Piglionico", "car park near Levigliani".
    pub name: String,
    /// Car park or village.
    pub kind: AccessKind,
    /// Where it is.
    pub position: (f64, f64),
    /// The network node it is attached to.
    pub node: usize,
}

/// One way between the start and the place, as walked up.
#[derive(Debug, Clone, PartialEq)]
pub struct Leg {
    /// Points from the start to the top.
    pub line: Vec<(f64, f64)>,
    /// (distance from the start in metres, elevation in metres).
    pub profile: Vec<(f64, f64)>,
    /// Routes followed, in order (e.g. "CAI 7", "CAI 126").
    pub via: Vec<String>,
    /// Their relation ids.
    pub route_ids: Vec<i64>,
    /// Hardest difficulty along it.
    pub difficulty: String,
    /// Length in metres.
    pub length_m: f64,
    /// Climbing walking up.
    pub ascent_m: f64,
    /// Descending walking up.
    pub descent_m: f64,
    /// Elevation at the start.
    pub start_ele: f64,
    /// Elevation at the top.
    pub top_ele: f64,
    /// Highest point.
    pub max_ele: f64,
    /// Network edges used, with the metres walked on each (to find a different way down).
    used: Vec<(usize, f64)>,
}

/// How the tour comes back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// Back the same way.
    OutAndBack,
    /// Down another way to the same start.
    Loop,
}

/// A round trip from a start to the place and back.
#[derive(Debug, Clone, PartialEq)]
pub struct Tour {
    /// Out-and-back or loop.
    pub shape: Shape,
    /// Where it starts and ends.
    pub start: Access,
    /// Way up.
    pub up: Leg,
    /// Way down for a loop (stored as walked up; walk it backwards).
    pub down: Option<Leg>,
    /// Round-trip distance in metres.
    pub length_m: f64,
    /// Round-trip climbing.
    pub ascent_m: f64,
    /// Round-trip descending.
    pub descent_m: f64,
    /// Walking time in minutes, DIN 33466, no breaks.
    pub duration_min: u32,
    /// Hardest difficulty of the tour.
    pub difficulty: String,
    /// Highest point reached.
    pub max_ele: f64,
}

/// The tours from `starts` to `place`, quickest first.
pub fn tours(
    network: &Network,
    starts: &[Access],
    place: (f64, f64),
    reach_m: f64,
    limit: usize,
) -> Vec<Tour> {
    let Some(target) = network.snap(place, reach_m) else {
        return Vec::new();
    };
    let reach = network.reach(target, MAX_ONE_WAY_H, &[], 1.0);

    let mut candidates: Vec<&Access> = starts
        .iter()
        .filter(|start| reach.hours[start.node].is_finite())
        .collect();
    candidates.sort_by(|a, b| {
        reach.hours[a.node]
            .total_cmp(&reach.hours[b.node])
            .then(a.kind.rank().cmp(&b.kind.rank()))
    });

    let mut tours: Vec<Tour> = Vec::new();
    for start in candidates {
        if tours.len() >= limit {
            break;
        }
        let too_close = tours
            .iter()
            .any(|t| network::distance(t.start.position, start.position) < DISTINCT_STARTS_M);
        if too_close {
            continue;
        }
        let Some(up) = leg(network, &reach, start.node) else {
            continue;
        };
        if up.top_ele <= up.start_ele {
            continue; // a walk down to the place, not up
        }
        tours.push(match different_way_down(network, target, start.node, &up) {
            Some(down) => loop_tour(start, up, down),
            None => out_and_back(start, up),
        });
    }
    tours.sort_by_key(|tour| tour.duration_min);
    tours
}

/// A second way between the start and the place that avoids the way up, if there is a good one.
fn different_way_down(
    network: &Network,
    target: network::Snap,
    start: usize,
    up: &Leg,
) -> Option<Leg> {
    let up_edges: Vec<usize> = up.used.iter().map(|&(edge, _)| edge).collect();
    let reach = network.reach(
        target,
        MAX_ONE_WAY_H * LOOP_MAX_LONGER,
        &up_edges,
        LOOP_PENALTY,
    );
    let other = leg(network, &reach, start)?;
    // Metres both ways walk on the same edges (a partial edge counts only the part walked)
    let shared: f64 = other
        .used
        .iter()
        .filter_map(|&(edge, metres)| {
            up.used
                .iter()
                .find(|&&(e, _)| e == edge)
                .map(|&(_, up_metres)| metres.min(up_metres))
        })
        .sum();
    let effort = |leg: &Leg| network::effort_hours(leg.length_m, leg.ascent_m, leg.descent_m);
    let different = shared <= LOOP_MAX_OVERLAP * up.length_m;
    (different && effort(&other) <= LOOP_MAX_LONGER * effort(up)).then_some(other)
}

/// The way from `start` to the target as a leg: the path's edges, then the partial stretch.
fn leg(network: &Network, reach: &Reach, start: usize) -> Option<Leg> {
    let steps = network.path(reach, start)?;
    let mut builder = LegBuilder::default();
    let mut node = start;
    for &Step { edge, forward } in &steps {
        let e = &network.edges[edge];
        let (line, profile): (Points, Points) = if forward {
            (e.line.clone(), e.profile.clone())
        } else {
            (
                e.line.iter().rev().copied().collect(),
                e.profile
                    .iter()
                    .rev()
                    .map(|&(d, el)| (e.length_m - d, el))
                    .collect(),
            )
        };
        builder.add(&line, &profile, edge, network);
        node = if forward { e.b } else { e.a };
    }
    // Last stretch: along the target's edge from `node` to the target point
    let target = reach.target;
    let e = &network.edges[target.edge];
    let (from_a, from_b) = network::split_edge(e, target.distance_m);
    let along = cumulative(&e.line);
    let (part, mut line): (Part, Vec<(f64, f64)>) = if node == e.a {
        let points = e
            .line
            .iter()
            .zip(&along)
            .take_while(|&(_, &d)| d < target.distance_m)
            .map(|(&p, _)| p)
            .collect();
        (from_a, points)
    } else {
        let points = e
            .line
            .iter()
            .zip(&along)
            .rev()
            .take_while(|&(_, &d)| d > target.distance_m)
            .map(|(&p, _)| p)
            .collect();
        (from_b, points)
    };
    line.push(target.point);
    builder.add_part(&line, &part, target.edge, network);
    builder.finish()
}

fn cumulative(line: &[(f64, f64)]) -> Vec<f64> {
    let mut total = 0.0;
    let mut out = vec![0.0];
    for pair in line.windows(2) {
        total += network::distance(pair[0], pair[1]);
        out.push(total);
    }
    out
}

#[derive(Default)]
struct LegBuilder {
    line: Vec<(f64, f64)>,
    profile: Vec<(f64, f64)>,
    via: Vec<String>,
    route_ids: Vec<i64>,
    difficulties: Vec<String>,
    used: Vec<(usize, f64)>,
}

impl LegBuilder {
    fn append(&mut self, line: &[(f64, f64)], profile: &[(f64, f64)]) {
        let offset = self.profile.last().map_or(0.0, |&(d, _)| d);
        let skip_point = usize::from(!self.line.is_empty());
        let skip_sample = usize::from(!self.profile.is_empty());
        self.line.extend(line.iter().skip(skip_point));
        self.profile.extend(
            profile
                .iter()
                .skip(skip_sample)
                .map(|&(d, e)| (offset + d, e)),
        );
    }

    fn add(&mut self, line: &[(f64, f64)], profile: &[(f64, f64)], edge: usize, network: &Network) {
        let metres = profile.last().map_or(0.0, |&(d, _)| d);
        self.append(line, profile);
        // A stretch of (almost) no length, where a way arrives at the target's own edge end,
        // walks none of that edge
        if metres >= 1.0 {
            self.used.push((edge, metres));
            self.note_routes(network, edge);
        }
    }

    fn add_part(&mut self, line: &[(f64, f64)], part: &Part, edge: usize, network: &Network) {
        let start = part.profile.first().map_or(0.0, |&(d, _)| d);
        let profile: Vec<(f64, f64)> = part.profile.iter().map(|&(d, e)| (d - start, e)).collect();
        self.add(line, &profile, edge, network);
    }

    /// Follows the same route through shared stretches when it can, so "via" stays short.
    fn note_routes(&mut self, network: &Network, edge: usize) {
        let routes = &network.edges[edge].routes;
        let current = self.route_ids.last().copied();
        // Keep following the current route; otherwise prefer a numbered CAI route over a
        // long-distance trail sharing the stretch ("CAI 126" says more than a 300 km Cammino)
        let route = routes
            .iter()
            .find(|r| Some(r.osm_id) == current)
            .or_else(|| routes.iter().find(|r| r.label.starts_with("CAI ")))
            .unwrap_or(&routes[0]);
        if Some(route.osm_id) != current {
            self.route_ids.push(route.osm_id);
            self.via.push(route.label.clone());
        }
        // The stretch's difficulty: the hardest known among its routes, else unknown
        let stretch = routes
            .iter()
            .map(|r| r.difficulty.as_str())
            .filter(|d| *d != "unknown")
            .fold(None::<String>, |acc, d| {
                Some(acc.map_or(d.to_owned(), |a| hardest(&a, d)))
            });
        self.difficulties
            .push(stretch.unwrap_or_else(|| "unknown".to_owned()));
    }

    fn finish(self) -> Option<Leg> {
        let (&(_, start_ele), &(length_m, top_ele)) = (self.profile.first()?, self.profile.last()?);
        let elevations: Vec<f64> = self.profile.iter().map(|&(_, e)| e).collect();
        let (ascent_m, descent_m) = network::climb(&elevations);
        let difficulty = self
            .difficulties
            .iter()
            .fold(None::<String>, |acc, d| {
                Some(acc.map_or(d.clone(), |a| hardest(&a, d)))
            })
            .unwrap_or_else(|| "unknown".to_owned());
        Some(Leg {
            line: self.line,
            profile: self.profile,
            via: self.via,
            route_ids: self.route_ids,
            difficulty,
            length_m,
            ascent_m,
            descent_m,
            start_ele,
            top_ele,
            max_ele: elevations.iter().copied().fold(f64::MIN, f64::max),
            used: self.used,
        })
    }
}

fn out_and_back(start: &Access, up: Leg) -> Tour {
    Tour {
        shape: Shape::OutAndBack,
        start: start.clone(),
        length_m: 2.0 * up.length_m,
        // Up climbs the ascent and the dips; back climbs out of the dips again
        ascent_m: up.ascent_m + up.descent_m,
        descent_m: up.ascent_m + up.descent_m,
        duration_min: walking_minutes(up.length_m, up.ascent_m, up.descent_m)
            + walking_minutes(up.length_m, up.descent_m, up.ascent_m),
        difficulty: up.difficulty.clone(),
        max_ele: up.max_ele,
        up,
        down: None,
    }
}

fn loop_tour(start: &Access, up: Leg, down: Leg) -> Tour {
    Tour {
        shape: Shape::Loop,
        start: start.clone(),
        length_m: up.length_m + down.length_m,
        // `down` is stored as walked up: walking it down swaps its climbs and descents
        ascent_m: up.ascent_m + down.descent_m,
        descent_m: up.descent_m + down.ascent_m,
        duration_min: walking_minutes(up.length_m, up.ascent_m, up.descent_m)
            + walking_minutes(down.length_m, down.descent_m, down.ascent_m),
        difficulty: hardest(&up.difficulty, &down.difficulty),
        max_ele: up.max_ele.max(down.max_ele),
        up,
        down: Some(down),
    }
}

/// The harder of two difficulties; "unknown" if either is unknown (never assume a stretch is easy).
pub fn hardest(a: &str, b: &str) -> String {
    let rank = |grade: &str| match grade {
        "T" => Some(0),
        "E" => Some(1),
        "EE" => Some(2),
        g if g.starts_with("EEA") => Some(3),
        _ => None,
    };
    match (rank(a), rank(b)) {
        (Some(x), Some(y)) => (if x >= y { a } else { b }).to_owned(),
        _ => "unknown".to_owned(),
    }
}

/// DIN 33466 walking time in minutes, rounded to 5 (same rule as the catalogue, aret-maps#5).
pub fn walking_minutes(length_m: f64, ascent_m: f64, descent_m: f64) -> u32 {
    let horizontal_h = length_m / 4000.0;
    let vertical_h = ascent_m / 300.0 + descent_m / 500.0;
    let hours = horizontal_h.max(vertical_h) + horizontal_h.min(vertical_h) / 2.0;
    ((hours * 60.0 / 5.0).round() * 5.0) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::tests::{route, straight};

    fn village(network: &Network, at: (f64, f64), name: &str) -> Access {
        Access {
            id: format!("node/{name}"),
            name: name.to_owned(),
            kind: AccessKind::Village,
            position: at,
            node: network
                .nodes
                .iter()
                .position(|&p| p == at)
                .expect("start on a node"),
        }
    }

    #[test]
    fn chains_routes_from_a_village_to_the_summit() {
        // CAI 1 from the village to a junction, CAI 2 from the junction to the summit
        let network = Network::build(&[
            straight(1, (10.3, 44.0), 5, 500.0, 50.0),
            straight(2, (10.3, 44.004), 6, 700.0, 60.0),
        ]);
        let start = village(&network, (10.3, 44.0), "Valley");
        let found = tours(&network, &[start], (10.3, 44.009), 200.0, 6);
        assert_eq!(found.len(), 1);
        let tour = &found[0];
        assert_eq!(tour.shape, Shape::OutAndBack);
        assert_eq!(tour.up.via, vec!["CAI 1", "CAI 2"]);
        assert_eq!(tour.up.start_ele, 500.0);
        assert_eq!(tour.up.top_ele, 1000.0);
        assert_eq!(tour.ascent_m, 500.0);
        assert!(
            (tour.length_m - 2.0 * 1000.6).abs() < 10.0,
            "{}",
            tour.length_m
        );
    }

    #[test]
    fn a_second_way_down_makes_a_loop() {
        // Two separate routes from the same village to the same summit
        let west = vec![
            (10.3, 44.0),
            (10.298, 44.003),
            (10.298, 44.006),
            (10.3, 44.009),
        ];
        let east = vec![
            (10.3, 44.0),
            (10.302, 44.003),
            (10.302, 44.006),
            (10.3, 44.009),
        ];
        let network = Network::build(&[route(1, west, 500.0, 150.0), route(2, east, 500.0, 150.0)]);
        let start = village(&network, (10.3, 44.0), "Valley");
        let found = tours(&network, &[start], (10.3, 44.009), 200.0, 6);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].shape, Shape::Loop);
        let down = found[0].down.as_ref().expect("a way down");
        let mut ways = vec![found[0].up.via[0].clone(), down.via[0].clone()];
        ways.sort();
        assert_eq!(ways, vec!["CAI 1", "CAI 2"]);
    }

    #[test]
    fn never_crosses_a_gap_between_routes() {
        // CAI 2 to the summit starts 40 m from where CAI 1 ends: they don't connect
        let network = Network::build(&[
            straight(1, (10.3, 44.0), 5, 500.0, 50.0),
            straight(2, (10.3005, 44.004), 6, 700.0, 60.0),
        ]);
        let start = village(&network, (10.3, 44.0), "Valley");
        assert!(tours(&network, &[start], (10.3005, 44.009), 200.0, 6).is_empty());
    }

    #[test]
    fn hardest_difficulty_and_unknown_wins() {
        assert_eq!(hardest("E", "EE"), "EE");
        assert_eq!(hardest("EEA:F", "E"), "EEA:F");
        assert_eq!(hardest("T", "unknown"), "unknown");
    }

    #[test]
    fn walking_time_is_din_33466() {
        // 1 km and +450 m: 1.5 h + 0.25 h / 2 = 1 h 37.5 → 1 h 40
        assert_eq!(walking_minutes(1000.0, 450.0, 0.0), 100);
    }
}
