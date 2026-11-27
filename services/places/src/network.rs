//! The network of marked trails: every pre-built route split at the points where routes meet
//! (aret-backend#24). Routes are joined **only where they share a point**: the same OpenStreetMap
//! node, hence the same coordinates in the catalogue. A route ending 40 m from another stays
//! unconnected, so a path through the network never bridges a gap (ADR-0015).
//!
//! Search runs from the target backwards, by walking effort: hours on the flat at 4 km/h plus
//! climbing at 300 m/h and descending at 500 m/h, added up per stretch (DIN 33466's own formula
//! isn't additive; the totals of a found path are timed with it afterwards).

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

use geo::{Distance, Haversine, Point};

/// Changes in the 50 m profiles counted as climbing from this size, in metres.
const CLIMB_THRESHOLD_M: f64 = 3.0;
/// Size of the grid cells used to find edges near a point, in degrees (~1 km).
const CELL_DEG: f64 = 0.01;

/// A route as stored in the catalogue, for building the network.
#[derive(Debug, Clone, PartialEq)]
pub struct RouteInput {
    /// OpenStreetMap relation id.
    pub osm_id: i64,
    /// "CAI 126", the name, or the relation id.
    pub label: String,
    /// `T`, `E`, `EE`, `EEA…` or `unknown`.
    pub difficulty: String,
    /// The route's lines (longitude, latitude).
    pub lines: Vec<Vec<(f64, f64)>>,
    /// Per line: (distance from the line's start in metres, elevation in metres).
    pub profiles: Vec<Vec<(f64, f64)>>,
}

/// A route an edge belongs to.
#[derive(Debug, Clone, PartialEq)]
pub struct RouteRef {
    /// OpenStreetMap relation id.
    pub osm_id: i64,
    /// "CAI 126", the name, or the relation id.
    pub label: String,
    /// The route's difficulty.
    pub difficulty: String,
}

/// A stretch of trail between two junctions (or a junction and an end).
#[derive(Debug, Clone, PartialEq)]
pub struct Edge {
    /// Node at the first point.
    pub a: usize,
    /// Node at the last point.
    pub b: usize,
    /// Points from `a` to `b`.
    pub line: Vec<(f64, f64)>,
    /// (distance from `a` in metres, elevation in metres), first and last point included.
    pub profile: Vec<(f64, f64)>,
    /// Length in metres.
    pub length_m: f64,
    /// Climbing and descending walking from `a` to `b`.
    pub ascent_m: f64,
    /// Descending walking from `a` to `b`.
    pub descent_m: f64,
    /// Routes running along it.
    pub routes: Vec<RouteRef>,
}

impl Edge {
    /// Walking effort from `a` to `b` (`forward`) or back, in hours.
    pub fn hours(&self, forward: bool) -> f64 {
        let (up, down) = if forward {
            (self.ascent_m, self.descent_m)
        } else {
            (self.descent_m, self.ascent_m)
        };
        effort_hours(self.length_m, up, down)
    }
}

/// Additive walking effort in hours.
pub fn effort_hours(length_m: f64, ascent_m: f64, descent_m: f64) -> f64 {
    length_m / 4000.0 + ascent_m / 300.0 + descent_m / 500.0
}

/// The trail network.
#[derive(Debug, Default)]
pub struct Network {
    /// Junctions and ends.
    pub nodes: Vec<(f64, f64)>,
    /// Stretches between nodes.
    pub edges: Vec<Edge>,
    /// Per node: (edge, whether the node is the edge's `a`).
    adjacency: Vec<Vec<(usize, bool)>>,
    /// Edges by grid cell of each of their points.
    grid: HashMap<(i64, i64), Vec<usize>>,
    /// Nodes by grid cell.
    node_grid: HashMap<(i64, i64), Vec<usize>>,
}

/// Where a point meets the network: on edge `edge`, `distance_m` from its `a`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Snap {
    /// Edge index.
    pub edge: usize,
    /// The closest point on the edge.
    pub point: (f64, f64),
    /// Distance from the edge's `a`, along it, in metres.
    pub distance_m: f64,
    /// How far the point is from the edge, in metres.
    pub offset_m: f64,
}

/// One step of a path: an edge walked in a direction (towards the target).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Step {
    /// Edge index.
    pub edge: usize,
    /// Walked from `a` to `b`.
    pub forward: bool,
}

/// Effort from every reachable node to the target, and the next step from each towards it.
#[derive(Debug, Clone)]
pub struct Reach {
    /// Hours from the node to the target; infinite if unreachable.
    pub hours: Vec<f64>,
    /// First step from the node towards the target (`None` at the target's neighbours, whose
    /// last stretch is the partial edge to the target).
    pub next: Vec<Option<Step>>,
    /// Where the target is on the network.
    pub target: Snap,
}

impl Network {
    /// Builds the network from the catalogue's routes.
    pub fn build(routes: &[RouteInput]) -> Self {
        let key = |(lon, lat): (f64, f64)| ((lon * 1e6).round() as i64, (lat * 1e6).round() as i64);

        // A point is a node where paths branch (its distinct neighbours aren't exactly two: a
        // junction, a crossing) or where a line ends. Points that routes merely share along the
        // same path have the same two neighbours and stay inside an edge.
        let mut neighbours: HashMap<(i64, i64), Vec<(i64, i64)>> = HashMap::new();
        let mut ends: std::collections::HashSet<(i64, i64)> = std::collections::HashSet::new();
        for route in routes {
            for line in &route.lines {
                if let (Some(&first), Some(&last)) = (line.first(), line.last()) {
                    ends.insert(key(first));
                    ends.insert(key(last));
                }
                for pair in line.windows(2) {
                    let (a, b) = (key(pair[0]), key(pair[1]));
                    for (from, to) in [(a, b), (b, a)] {
                        let list = neighbours.entry(from).or_default();
                        if !list.contains(&to) {
                            list.push(to);
                        }
                    }
                }
            }
        }
        let is_junction = |point: (f64, f64)| {
            let k = key(point);
            ends.contains(&k) || neighbours.get(&k).is_none_or(|list| list.len() != 2)
        };
        let mut network = Self::default();
        let mut node_ids: HashMap<(i64, i64), usize> = HashMap::new();
        let mut node = |network: &mut Self, point: (f64, f64)| -> usize {
            *node_ids.entry(key(point)).or_insert_with(|| {
                network.nodes.push(point);
                network.adjacency.push(Vec::new());
                network.nodes.len() - 1
            })
        };
        // The same stretch shared by several routes becomes one edge listing them all. "Same"
        // means the same two nodes and the same path between them: the point next to the
        // lower-numbered node (two different paths can have the same length)
        type EdgeKey = (usize, usize, (i64, i64));
        let mut edge_ids: HashMap<EdgeKey, usize> = HashMap::new();

        for route in routes {
            let reference = RouteRef {
                osm_id: route.osm_id,
                label: route.label.clone(),
                difficulty: route.difficulty.clone(),
            };
            for (index, line) in route.lines.iter().enumerate() {
                if line.len() < 2 {
                    continue;
                }
                let profile = route.profiles.get(index).cloned().unwrap_or_default();
                let distances = cumulative(line);
                let mut start = 0;
                for i in 1..line.len() {
                    let is_node = i == line.len() - 1 || is_junction(line[i]);
                    if !is_node {
                        continue;
                    }
                    let a = node(&mut network, line[start]);
                    let b = node(&mut network, line[i]);
                    let length = distances[i] - distances[start];
                    let piece = &line[start..=i];
                    let next_to_first = if a <= b {
                        piece[1]
                    } else {
                        piece[piece.len() - 2]
                    };
                    let dedupe = (a.min(b), a.max(b), key(next_to_first));
                    if let Some(&existing) = edge_ids.get(&dedupe) {
                        if !network.edges[existing]
                            .routes
                            .iter()
                            .any(|r| r.osm_id == route.osm_id)
                        {
                            network.edges[existing].routes.push(reference.clone());
                        }
                    } else if length > 0.0 {
                        let edge_profile = slice_profile(&profile, distances[start], distances[i]);
                        let elevations: Vec<f64> = edge_profile.iter().map(|&(_, e)| e).collect();
                        let (ascent_m, descent_m) = climb(&elevations);
                        let id = network.edges.len();
                        network.edges.push(Edge {
                            a,
                            b,
                            line: line[start..=i].to_vec(),
                            profile: edge_profile,
                            length_m: length,
                            ascent_m,
                            descent_m,
                            routes: vec![reference.clone()],
                        });
                        network.adjacency[a].push((id, true));
                        network.adjacency[b].push((id, false));
                        for &point in &line[start..=i] {
                            let cell = cell(point);
                            let cell_edges = network.grid.entry(cell).or_default();
                            if cell_edges.last() != Some(&id) {
                                cell_edges.push(id);
                            }
                        }
                        edge_ids.insert(dedupe, id);
                    }
                    start = i;
                }
            }
        }
        for (id, &point) in network.nodes.iter().enumerate() {
            network.node_grid.entry(cell(point)).or_default().push(id);
        }
        network
    }

    /// The nearest junction or route end to `point`, if within `max_m` metres (under ~1 km).
    pub fn nearest_node(&self, point: (f64, f64), max_m: f64) -> Option<(usize, f64)> {
        let (x, y) = cell(point);
        (x - 1..=x + 1)
            .flat_map(|cx| (y - 1..=y + 1).map(move |cy| (cx, cy)))
            .filter_map(|c| self.node_grid.get(&c))
            .flatten()
            .map(|&node| (node, distance(point, self.nodes[node])))
            .filter(|&(_, d)| d <= max_m)
            .min_by(|a, b| a.1.total_cmp(&b.1))
    }

    /// The nearest point of the network to `point`, if within `max_m` metres (under ~1 km).
    pub fn snap(&self, point: (f64, f64), max_m: f64) -> Option<Snap> {
        let (x, y) = cell(point);
        let mut best: Option<Snap> = None;
        let mut seen = std::collections::HashSet::new();
        for cx in x - 1..=x + 1 {
            for cy in y - 1..=y + 1 {
                for &edge in self.grid.get(&(cx, cy)).into_iter().flatten() {
                    if !seen.insert(edge) {
                        continue;
                    }
                    let Some(near) = nearest_on_line(point, &self.edges[edge].line) else {
                        continue;
                    };
                    if near.offset_m <= max_m && best.is_none_or(|b| near.offset_m < b.offset_m) {
                        best = Some(Snap {
                            edge,
                            point: near.point,
                            distance_m: near.distance_m,
                            offset_m: near.offset_m,
                        });
                    }
                }
            }
        }
        best
    }

    /// Effort from every node to `target`, up to `max_hours`; edges in `penalised` cost
    /// `penalty` times more (to find a different way back).
    pub fn reach(&self, target: Snap, max_hours: f64, penalised: &[usize], penalty: f64) -> Reach {
        let mut hours = vec![f64::INFINITY; self.nodes.len()];
        let mut next: Vec<Option<Step>> = vec![None; self.nodes.len()];
        let mut queue = BinaryHeap::new();
        let edge = &self.edges[target.edge];
        // Partial stretches of the target's edge: from `a` up to the target, from `b` back to it
        let (to_a, to_b) = split_edge(edge, target.distance_m);
        for (node, part) in [(edge.a, &to_a), (edge.b, &to_b)] {
            let cost = effort_hours(part.length_m, part.ascent_m, part.descent_m)
                * if penalised.contains(&target.edge) {
                    penalty
                } else {
                    1.0
                };
            if cost < hours[node] {
                hours[node] = cost;
                queue.push(Entry { hours: cost, node });
            }
        }
        while let Some(Entry { hours: at, node }) = queue.pop() {
            if at > hours[node] || at > max_hours {
                continue;
            }
            for &(edge_id, node_is_a) in &self.adjacency[node] {
                let edge = &self.edges[edge_id];
                // Walking from the other end towards `node`
                let (other, forward) = if node_is_a {
                    (edge.b, false)
                } else {
                    (edge.a, true)
                };
                let factor = if penalised.contains(&edge_id) {
                    penalty
                } else {
                    1.0
                };
                let cost = at + edge.hours(forward) * factor;
                if cost < hours[other] {
                    hours[other] = cost;
                    next[other] = Some(Step {
                        edge: edge_id,
                        forward,
                    });
                    queue.push(Entry {
                        hours: cost,
                        node: other,
                    });
                }
            }
        }
        Reach {
            hours,
            next,
            target,
        }
    }

    /// The path from `start` to the target as steps, then the final partial stretch.
    pub fn path(&self, reach: &Reach, start: usize) -> Option<Vec<Step>> {
        if !reach.hours[start].is_finite() {
            return None;
        }
        let mut steps = Vec::new();
        let mut node = start;
        let target_edge = &self.edges[reach.target.edge];
        while let Some(step) = reach.next[node] {
            steps.push(step);
            let edge = &self.edges[step.edge];
            node = if step.forward { edge.b } else { edge.a };
            if steps.len() > self.edges.len() {
                return None; // can't happen with consistent `next`; never loop forever
            }
        }
        // The last node touches the target's edge
        (node == target_edge.a || node == target_edge.b).then_some(steps)
    }
}

/// An edge cut at `distance` from `a`: (part from `a` to the cut, part from `b` back to the cut).
pub fn split_edge(edge: &Edge, distance: f64) -> (Part, Part) {
    let cut_ele = elevation_at(&edge.profile, distance);
    let mut first: Vec<(f64, f64)> = edge
        .profile
        .iter()
        .copied()
        .take_while(|&(d, _)| d < distance)
        .collect();
    first.extend(cut_ele.map(|e| (distance, e)));
    let mut second: Vec<(f64, f64)> = cut_ele.map(|e| (distance, e)).into_iter().collect();
    second.extend(
        edge.profile
            .iter()
            .copied()
            .skip_while(|&(d, _)| d <= distance),
    );
    let second: Vec<(f64, f64)> = second
        .into_iter()
        .rev()
        .map(|(d, e)| (edge.length_m - d, e))
        .collect();
    (Part::new(first), Part::new(second))
}

/// A partial stretch: its profile from where it's walked from, with totals.
#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    /// (distance in metres, elevation in metres).
    pub profile: Vec<(f64, f64)>,
    /// Length in metres.
    pub length_m: f64,
    /// Climbing.
    pub ascent_m: f64,
    /// Descending.
    pub descent_m: f64,
}

impl Part {
    fn new(profile: Vec<(f64, f64)>) -> Self {
        let length_m = match (profile.first(), profile.last()) {
            (Some(first), Some(last)) => last.0 - first.0,
            _ => 0.0,
        };
        let elevations: Vec<f64> = profile.iter().map(|&(_, e)| e).collect();
        let (ascent_m, descent_m) = climb(&elevations);
        Self {
            profile,
            length_m,
            ascent_m,
            descent_m,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Entry {
    hours: f64,
    node: usize,
}

impl Eq for Entry {}

impl Ord for Entry {
    // Smallest effort first (BinaryHeap is a max-heap)
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .hours
            .total_cmp(&self.hours)
            .then(self.node.cmp(&other.node))
    }
}

impl PartialOrd for Entry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn cell((lon, lat): (f64, f64)) -> (i64, i64) {
    (
        (lon / CELL_DEG).floor() as i64,
        (lat / CELL_DEG).floor() as i64,
    )
}

/// Distance in metres on a sphere: within ~0.3% of the ellipsoid for trail segments, and far
/// faster (building the network measures over a million of them).
pub fn distance(a: (f64, f64), b: (f64, f64)) -> f64 {
    Haversine.distance(Point::from(a), Point::from(b))
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

/// Profile between two distances along a line, re-based to start at 0.
fn slice_profile(profile: &[(f64, f64)], from: f64, to: f64) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    if let Some(e) = elevation_at(profile, from) {
        out.push((0.0, e));
    }
    out.extend(
        profile
            .iter()
            .filter(|&&(d, _)| d > from && d < to)
            .map(|&(d, e)| (d - from, e)),
    );
    if let Some(e) = elevation_at(profile, to) {
        out.push((to - from, e));
    }
    out
}

fn elevation_at(profile: &[(f64, f64)], distance: f64) -> Option<f64> {
    let last = profile.last()?;
    if distance >= last.0 {
        return Some(last.1);
    }
    let after = profile.iter().position(|&(d, _)| d >= distance)?;
    if after == 0 {
        return Some(profile[0].1);
    }
    let ((d0, e0), (d1, e1)) = (profile[after - 1], profile[after]);
    let t = if d1 > d0 {
        (distance - d0) / (d1 - d0)
    } else {
        0.0
    };
    Some(e0 + (e1 - e0) * t)
}

/// Ascent and descent with a small threshold.
pub fn climb(elevations: &[f64]) -> (f64, f64) {
    let Some(&first) = elevations.first() else {
        return (0.0, 0.0);
    };
    let (mut up, mut down, mut reference) = (0.0, 0.0, first);
    for &ele in &elevations[1..] {
        let change = ele - reference;
        if change >= CLIMB_THRESHOLD_M {
            up += change;
            reference = ele;
        } else if change <= -CLIMB_THRESHOLD_M {
            down -= change;
            reference = ele;
        }
    }
    (up, down)
}

/// Closest point of a line to a point.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Near {
    point: (f64, f64),
    distance_m: f64,
    offset_m: f64,
}

fn nearest_on_line(target: (f64, f64), line: &[(f64, f64)]) -> Option<Near> {
    let mut best: Option<Near> = None;
    let mut walked = 0.0;
    // Flat projection around the target: fine for the short segments of a trail
    let scale = target.1.to_radians().cos();
    for pair in line.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let (ax, ay) = ((a.0 - target.0) * scale, a.1 - target.1);
        let (dx, dy) = ((b.0 - a.0) * scale, b.1 - a.1);
        let length_sq = dx * dx + dy * dy;
        let t = if length_sq > 0.0 {
            (-(ax * dx + ay * dy) / length_sq).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let point = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
        let offset = distance(point, target);
        if best.is_none_or(|n| offset < n.offset_m) {
            best = Some(Near {
                point,
                distance_m: walked + distance(a, point),
                offset_m: offset,
            });
        }
        walked += distance(a, b);
    }
    best
}

/// Test helpers, also used by the tours tests.
#[cfg(test)]
pub mod tests {
    use super::*;

    /// A straight line north from (`lon`, `lat`), `n` points every 0.001° (~111 m), climbing
    /// `step` metres per point from `ele0`, as a route.
    pub fn straight(osm_id: i64, from: (f64, f64), n: usize, ele0: f64, step: f64) -> RouteInput {
        let line: Vec<(f64, f64)> = (0..n)
            .map(|i| (from.0, from.1 + 0.001 * i as f64))
            .collect();
        route(osm_id, line, ele0, step)
    }

    /// A route along `line`, climbing `step` metres per point.
    pub fn route(osm_id: i64, line: Vec<(f64, f64)>, ele0: f64, step: f64) -> RouteInput {
        let distances = cumulative(&line);
        let profile = distances
            .iter()
            .enumerate()
            .map(|(i, &d)| (d, ele0 + step * i as f64))
            .collect();
        RouteInput {
            osm_id,
            label: format!("CAI {osm_id}"),
            difficulty: "E".to_owned(),
            lines: vec![line],
            profiles: vec![profile],
        }
    }

    #[test]
    fn joins_routes_only_where_they_share_a_point() {
        // Route 1 ends at (10.3, 44.004); route 2 starts there; route 3 starts 40 m away
        let first = straight(1, (10.3, 44.0), 5, 500.0, 20.0);
        let second = straight(2, (10.3, 44.004), 5, 580.0, 20.0);
        let third = straight(3, (10.3005, 44.004), 5, 580.0, 20.0);
        let network = Network::build(&[first, second, third]);
        let junction = network
            .nodes
            .iter()
            .position(|&p| p == (10.3, 44.004))
            .expect("shared point is a node");
        assert_eq!(
            network.adjacency[junction].len(),
            2,
            "routes 1 and 2 meet there"
        );
        let near_third = network
            .nodes
            .iter()
            .position(|&p| p == (10.3005, 44.004))
            .expect("node");
        assert_eq!(
            network.adjacency[near_third].len(),
            1,
            "route 3 is not joined to them"
        );
    }

    #[test]
    fn splits_a_route_where_another_joins_it_midway() {
        let main = straight(1, (10.3, 44.0), 10, 500.0, 10.0);
        // A spur starting at the 5th point of the main route, going east
        let spur_line: Vec<(f64, f64)> =
            (0..4).map(|i| (10.3 + 0.001 * i as f64, 44.004)).collect();
        let network = Network::build(&[main, route(2, spur_line, 540.0, 15.0)]);
        assert_eq!(network.edges.len(), 3, "main split in two, plus the spur");
    }

    #[test]
    fn keeps_two_paths_of_equal_length_apart() {
        // Mirror-image paths between the same two points: two edges, not one
        let west = vec![(10.3, 44.0), (10.298, 44.004), (10.3, 44.008)];
        let east = vec![(10.3, 44.0), (10.302, 44.004), (10.3, 44.008)];
        let network = Network::build(&[route(1, west, 500.0, 50.0), route(2, east, 500.0, 50.0)]);
        assert_eq!(network.edges.len(), 2);
        // ... while a route following the very same path (here backwards) shares its one edge
        let again = vec![(10.3, 44.0), (10.298, 44.004), (10.3, 44.008)];
        let shared = Network::build(&[
            route(
                1,
                vec![(10.3, 44.0), (10.298, 44.004), (10.3, 44.008)],
                500.0,
                50.0,
            ),
            route(3, again.into_iter().rev().collect(), 600.0, -50.0),
        ]);
        assert_eq!(shared.edges.len(), 1);
        assert_eq!(shared.edges[0].routes.len(), 2);
    }

    #[test]
    fn finds_the_way_through_junctions() {
        // Village at the bottom of route 1, which meets route 2 to the summit
        let first = straight(1, (10.3, 44.0), 5, 500.0, 50.0);
        let second = straight(2, (10.3, 44.004), 6, 700.0, 60.0);
        let network = Network::build(&[first, second]);
        let summit = network.snap((10.3, 44.009), 200.0).expect("on the network");
        let reach = network.reach(summit, 10.0, &[], 1.0);
        let village = network
            .nodes
            .iter()
            .position(|&p| p == (10.3, 44.0))
            .expect("node");
        assert!(reach.hours[village].is_finite());
        let steps = network.path(&reach, village).expect("a path");
        assert_eq!(
            steps.len(),
            1,
            "route 1 to the junction, then route 2's stretch to the top"
        );
    }
}
