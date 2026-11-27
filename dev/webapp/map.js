// Map of the pilot region with Aret's own vector tiles (OpenMapTiles schema) from Cloudflare Pages.
// A first, simple outdoor style; the real style will live in aret-maps.
import {
  Map as MapLibreMap,
  NavigationControl,
  Popup,
  ScaleControl,
} from "https://cdn.jsdelivr.net/npm/maplibre-gl@6.18.0/dist/maplibre-gl.mjs";

const PILOT_BOUNDS = [9.29, 43.79, 11.38, 44.8]; // aret-maps/config/pilot-region.env
const FONT = ["Noto Sans Regular"];
const NAME = ["coalesce", ["get", "name:it"], ["get", "name"]];

// Pre-built routes (aret-maps#4, ADR-0015): OpenStreetMap hiking relations, mostly CAI trails.
// Colour = difficulty, only from tags; dashed = incomplete (gaps are never bridged).
export const DIFFICULTY_COLORS = {
  T: "#2e9d4b", // turistico
  E: "#d7301f", // escursionistico
  EE: "#1f3a93", // per escursionisti esperti
  EEA: "#7b3fa0", // attrezzato (EEA, EEA:F, EEA:PD, EEA:D)
  unknown: "#8a8a8a",
};
const ROUTE_COLOR = [
  "case",
  ["==", ["slice", ["get", "difficulty"], 0, 3], "EEA"], DIFFICULTY_COLORS.EEA,
  [
    "match", ["get", "difficulty"],
    "T", DIFFICULTY_COLORS.T,
    "E", DIFFICULTY_COLORS.E,
    "EE", DIFFICULTY_COLORS.EE,
    DIFFICULTY_COLORS.unknown,
  ],
];
const ROUTE_WIDTH = ["interpolate", ["linear"], ["zoom"], 9, 1, 12, 2.5, 15, 4];
// MapLibre only allows "zoom" at the top of an expression, so the highlight has its own scale
const SELECTED_WIDTH = ["interpolate", ["linear"], ["zoom"], 9, 4, 12, 8, 15, 12];

const style = {
  version: 8,
  // Fonts for labels (OpenFreeMap's public font server, free, no key)
  glyphs: "https://tiles.openfreemap.org/fonts/{fontstack}/{range}.pbf",
  sources: {
    aret: {
      type: "vector",
      tiles: ["https://aret-tiles.pages.dev/tiles/{z}/{x}/{y}.pbf"],
      minzoom: 0,
      maxzoom: 14,
      bounds: PILOT_BOUNDS,
      // Required credit for OpenMapTiles-schema tiles (aret-maps README)
      attribution:
        '© <a href="https://openmaptiles.org/" target="_blank">OpenMapTiles</a> ' +
        '© <a href="https://www.openstreetmap.org/copyright" target="_blank">OpenStreetMap contributors</a>',
    },
    routes: {
      type: "vector",
      tiles: ["https://aret-tiles.pages.dev/routes/{z}/{x}/{y}.pbf"],
      minzoom: 9,
      maxzoom: 14,
      bounds: PILOT_BOUNDS,
      attribution:
        'Routes © <a href="https://www.openstreetmap.org/copyright" target="_blank">OpenStreetMap contributors</a> (ODbL); ' +
        'elevation <a href="https://doi.org/10.13127/tinitaly/1.1" target="_blank">TINITALY/INGV</a> (CC BY 4.0)',
    },
    pilot: {
      type: "geojson",
      data: {
        type: "Feature",
        properties: {},
        geometry: {
          type: "Polygon",
          coordinates: [[
            [PILOT_BOUNDS[0], PILOT_BOUNDS[1]],
            [PILOT_BOUNDS[2], PILOT_BOUNDS[1]],
            [PILOT_BOUNDS[2], PILOT_BOUNDS[3]],
            [PILOT_BOUNDS[0], PILOT_BOUNDS[3]],
            [PILOT_BOUNDS[0], PILOT_BOUNDS[1]],
          ]],
        },
      },
    },
  },
  layers: [
    { id: "background", type: "background", paint: { "background-color": "#f1eee7" } },
    {
      id: "landcover",
      type: "fill",
      source: "aret",
      "source-layer": "landcover",
      paint: {
        "fill-color": [
          "match", ["get", "class"],
          "wood", "#c4d9a7",
          "grass", "#dde8c4",
          "farmland", "#ece9d6",
          "rock", "#dcd7cf",
          "ice", "#f5f9fb",
          "sand", "#efe5c8",
          "wetland", "#d3e4dc",
          "#e6e6d8",
        ],
        "fill-opacity": 0.8,
      },
    },
    {
      id: "park",
      type: "fill",
      source: "aret",
      "source-layer": "park",
      paint: { "fill-color": "#3c8a3c", "fill-opacity": 0.08, "fill-outline-color": "#3c8a3c" },
    },
    {
      id: "residential",
      type: "fill",
      source: "aret",
      "source-layer": "landuse",
      filter: ["in", ["get", "class"], ["literal", ["residential", "suburb", "neighbourhood"]]],
      paint: { "fill-color": "#e4ddd3" },
    },
    { id: "water", type: "fill", source: "aret", "source-layer": "water", paint: { "fill-color": "#a6c8e6" } },
    {
      id: "waterway",
      type: "line",
      source: "aret",
      "source-layer": "waterway",
      paint: { "line-color": "#a6c8e6", "line-width": ["interpolate", ["linear"], ["zoom"], 10, 0.5, 14, 2] },
    },
    {
      id: "boundary",
      type: "line",
      source: "aret",
      "source-layer": "boundary",
      filter: ["<=", ["get", "admin_level"], 6],
      paint: { "line-color": "#9a8fb0", "line-dasharray": [3, 2], "line-width": 1 },
    },
    {
      id: "roads",
      type: "line",
      source: "aret",
      "source-layer": "transportation",
      filter: ["!", ["in", ["get", "class"], ["literal", ["path", "track"]]]],
      layout: { "line-cap": "round", "line-join": "round" },
      paint: {
        "line-color": [
          "match", ["get", "class"],
          ["motorway", "trunk"], "#f0a85a",
          ["primary", "secondary"], "#f6cf7a",
          "#ffffff",
        ],
        "line-width": ["interpolate", ["linear"], ["zoom"], 8, 0.5, 14, 4],
      },
    },
    {
      id: "tracks",
      type: "line",
      source: "aret",
      "source-layer": "transportation",
      filter: ["==", ["get", "class"], "track"],
      minzoom: 11,
      paint: { "line-color": "#9a7446", "line-width": 1.2, "line-dasharray": [3, 1.5] },
    },
    {
      // Hiking paths in Aret orange, dashed like on paper maps
      id: "paths",
      type: "line",
      source: "aret",
      "source-layer": "transportation",
      filter: ["==", ["get", "class"], "path"],
      minzoom: 11,
      paint: {
        "line-color": "#e8501a",
        "line-width": ["interpolate", ["linear"], ["zoom"], 11, 0.8, 15, 2],
        "line-dasharray": [2, 1.5],
      },
    },
    {
      // The selected route, wide and white underneath so it stands out
      id: "route-selected",
      type: "line",
      source: "routes",
      "source-layer": "routes",
      filter: ["==", ["get", "osm_id"], -1],
      layout: { "line-cap": "round", "line-join": "round" },
      paint: { "line-color": "#ffffff", "line-width": SELECTED_WIDTH },
    },
    {
      id: "routes",
      type: "line",
      source: "routes",
      "source-layer": "routes",
      filter: ["get", "complete"],
      layout: { "line-cap": "round", "line-join": "round" },
      paint: { "line-color": ROUTE_COLOR, "line-width": ROUTE_WIDTH, "line-opacity": 0.85 },
    },
    {
      id: "routes-incomplete",
      type: "line",
      source: "routes",
      "source-layer": "routes",
      filter: ["!", ["get", "complete"]],
      paint: {
        "line-color": ROUTE_COLOR,
        "line-width": ROUTE_WIDTH,
        "line-opacity": 0.85,
        "line-dasharray": [1.5, 1.5],
      },
    },
    {
      // CAI number along the line, like the red-white waymarks
      id: "route-refs",
      type: "symbol",
      source: "routes",
      "source-layer": "routes",
      minzoom: 12,
      filter: ["has", "ref"],
      layout: {
        "symbol-placement": "line",
        "symbol-spacing": 250,
        "text-field": ["get", "ref"],
        "text-font": FONT,
        "text-size": 11,
      },
      paint: { "text-color": ROUTE_COLOR, "text-halo-color": "#fff", "text-halo-width": 2 },
    },
    {
      id: "pilot-area",
      type: "line",
      source: "pilot",
      paint: { "line-color": "#ff5717", "line-width": 2, "line-opacity": 0.6 },
    },
    {
      id: "places",
      type: "symbol",
      source: "aret",
      "source-layer": "place",
      filter: ["in", ["get", "class"], ["literal", ["city", "town", "village"]]],
      layout: {
        "text-field": NAME,
        "text-font": FONT,
        "text-size": ["match", ["get", "class"], "city", 15, "town", 13, 11],
      },
      paint: { "text-color": "#333", "text-halo-color": "#fff", "text-halo-width": 1.5 },
    },
    {
      id: "peaks",
      type: "symbol",
      source: "aret",
      "source-layer": "mountain_peak",
      layout: {
        "text-field": [
          "concat", "▲ ", NAME,
          ["case", ["has", "ele"], ["concat", "\n", ["to-string", ["get", "ele"]], " m"], ""],
        ],
        "text-font": FONT,
        "text-size": 11,
        "text-anchor": "top",
      },
      paint: { "text-color": "#5b3a1e", "text-halo-color": "#fff", "text-halo-width": 1.5 },
    },
  ],
};

const map = new MapLibreMap({
  container: "map",
  style,
  center: [10.335, 44.295],
  zoom: 9,
  hash: true, // the URL keeps the position: reload or share a view
});
map.addControl(new NavigationControl(), "top-right");
map.addControl(new ScaleControl({ unit: "metric" }), "bottom-left");

const mapReady = new Promise((resolve) => map.once("load", resolve));

// ---- route details on click

const ROUTE_LAYERS = ["routes", "routes-incomplete"];
const escape = (text) =>
  String(text).replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`);

for (const layer of ROUTE_LAYERS) {
  map.on("mouseenter", layer, () => (map.getCanvas().style.cursor = "pointer"));
  map.on("mouseleave", layer, () => (map.getCanvas().style.cursor = ""));
}

map.on("click", (event) => {
  const [feature] = map.queryRenderedFeatures(event.point, { layers: ROUTE_LAYERS });
  if (!feature) {
    map.setFilter("route-selected", ["==", ["get", "osm_id"], -1]);
    return;
  }
  const route = feature.properties;
  map.setFilter("route-selected", ["==", ["get", "osm_id"], route.osm_id]);
  // Never invent a name: routes with only a CAI number are shown as "CAI <ref>"
  const title = route.name ?? (route.ref ? `CAI ${route.ref}` : "Unnamed route");
  const ref = route.name && route.ref ? ` <span class="route-ref">${escape(route.ref)}</span>` : "";
  const difficulty = route.difficulty === "unknown" ? "difficulty unknown" : `difficulty ${escape(route.difficulty)}`;
  const status = route.complete
    ? ""
    : '<p class="route-warning">⚠ Incomplete in OpenStreetMap: shown in pieces, gaps are not joined.</p>';
  // Elevation (aret-maps#5). Missing when the route leaves the elevation model's area: vector tiles
  // drop empty properties. A route is drawn from its lower end, so "up" starts in the valley.
  const metres = (value) => Math.round(Number(value)).toLocaleString("en");
  const climb =
    route.ascent_m != null
      ? `<br>↗ ${metres(route.ascent_m)} m · ↘ ${metres(route.descent_m)} m · highest ${metres(route.ele_max)} m`
      : "";
  const hours = (minutes) => {
    const m = Number(minutes);
    return m < 60 ? `${m} min` : `${Math.floor(m / 60)} h ${String(m % 60).padStart(2, "0")}`;
  };
  const time =
    route.duration_min != null
      ? `<br>⏱ ~${hours(route.duration_min)} up · ~${hours(route.duration_reverse_min)} down ` +
        '<span class="route-note">(estimate, no breaks)</span>'
      : "";
  new Popup({ maxWidth: "280px" })
    .setLngLat(event.lngLat)
    .setHTML(
      `<strong>${escape(title)}</strong>${ref}<br>` +
        `${(route.length_m / 1000).toFixed(1)} km · ${difficulty}${route.network ? ` · ${escape(route.network)}` : ""}` +
        climb +
        time +
        status +
        `<p><a href="https://www.openstreetmap.org/relation/${Number(route.osm_id)}" target="_blank">View on OpenStreetMap</a></p>`,
    )
    .addTo(map);
});

// ---- tours (places-svc, aret-backend#24)

/** One colour per tour. */
export const TOUR_COLORS = ["#e6194b", "#3cb44b", "#4363d8", "#f58231", "#911eb4", "#00a3a3", "#f032e6", "#9a6324", "#808000", "#000075"];
const BASE_ROUTE_LAYERS = ["routes", "routes-incomplete", "route-refs", "route-selected"];

function boundsOf(coordinates) {
  const lons = coordinates.map((c) => c[0]);
  const lats = coordinates.map((c) => c[1]);
  return [[Math.min(...lons), Math.min(...lats)], [Math.max(...lons), Math.max(...lats)]];
}

async function ensureTourLayers() {
  await mapReady;
  if (map.getSource("tours")) return;
  const empty = { type: "FeatureCollection", features: [] };
  map.addSource("tours", {
    type: "geojson",
    data: empty,
    // Tours come from OSM routes and their times from TINITALY elevation: both must be credited
    attribution:
      'Tours © <a href="https://www.openstreetmap.org/copyright" target="_blank">OpenStreetMap contributors</a> (ODbL); ' +
      'elevation <a href="https://doi.org/10.13127/tinitaly/1.1" target="_blank">TINITALY/INGV</a> (CC BY 4.0)',
  });
  map.addSource("tour-starts", { type: "geojson", data: empty });
  map.addSource("tour-place", { type: "geojson", data: empty });
  map.addLayer({
    id: "tours-casing",
    type: "line",
    source: "tours",
    layout: { "line-cap": "round", "line-join": "round" },
    paint: { "line-color": "#ffffff", "line-width": ["case", ["get", "selected"], 9, 6], "line-opacity": ["get", "opacity"] },
  });
  map.addLayer({
    id: "tours-line",
    type: "line",
    source: "tours",
    layout: { "line-cap": "round", "line-join": "round" },
    paint: {
      "line-color": ["get", "color"],
      "line-width": ["case", ["get", "selected"], 5, 3.5],
      "line-opacity": ["get", "opacity"],
    },
  });
  map.addLayer({
    id: "tour-starts",
    type: "circle",
    source: "tour-starts",
    paint: { "circle-radius": 11, "circle-color": ["get", "color"], "circle-stroke-color": "#fff", "circle-stroke-width": 2, "circle-opacity": ["get", "opacity"], "circle-stroke-opacity": ["get", "opacity"] },
  });
  map.addLayer({
    id: "tour-start-numbers",
    type: "symbol",
    source: "tour-starts",
    layout: { "text-field": ["get", "number"], "text-font": FONT, "text-size": 12, "text-allow-overlap": true },
    paint: { "text-color": "#ffffff", "text-opacity": ["get", "opacity"] },
  });
  map.addLayer({
    id: "tour-place",
    type: "circle",
    source: "tour-place",
    paint: { "circle-radius": 7, "circle-color": "#ffffff", "circle-stroke-color": "#111", "circle-stroke-width": 3 },
  });
}

/**
 * Shows only `tours` (from GET /v1/tours) on the map: each in its colour with a numbered start,
 * all other routes hidden. With `selected` (an index), that tour stands out and the map zooms to it.
 */
export async function showTours(place, tours, selected = null) {
  await ensureTourLayers();
  for (const layer of BASE_ROUTE_LAYERS) map.setLayoutProperty(layer, "visibility", "none");
  const opacity = (index) => (selected === null || selected === index ? 1 : 0.25);
  map.getSource("tours").setData({
    type: "FeatureCollection",
    features: tours.map((tour, index) => ({
      type: "Feature",
      geometry: tour.geometry,
      properties: { color: TOUR_COLORS[index % TOUR_COLORS.length], selected: selected === index, opacity: opacity(index) },
    })),
  });
  map.getSource("tour-starts").setData({
    type: "FeatureCollection",
    features: tours.map((tour, index) => ({
      type: "Feature",
      geometry: { type: "Point", coordinates: tour.start.position },
      properties: { number: String(index + 1), color: TOUR_COLORS[index % TOUR_COLORS.length], opacity: opacity(index) },
    })),
  });
  map.getSource("tour-place").setData({
    type: "FeatureCollection",
    features: [{ type: "Feature", properties: {}, geometry: { type: "Point", coordinates: place.position } }],
  });
  const chosen = selected === null ? tours : [tours[selected]];
  const points = [place.position, ...chosen.flatMap((tour) => tour.geometry.coordinates.flat())];
  map.fitBounds(boundsOf(points), { padding: 50, maxZoom: 15, duration: 800 });
}

async function ensureDetailLayers() {
  await ensureTourLayers();
  if (map.getSource("tour-waypoints")) return;
  const empty = { type: "FeatureCollection", features: [] };
  map.addSource("tour-waypoints", { type: "geojson", data: empty });
  map.addSource("tour-hover", { type: "geojson", data: empty });
  map.addLayer({
    id: "tour-waypoints",
    type: "circle",
    source: "tour-waypoints",
    paint: { "circle-radius": 4, "circle-color": "#ffffff", "circle-stroke-color": "#333", "circle-stroke-width": 1.5 },
  });
  map.addLayer({
    id: "tour-waypoint-names",
    type: "symbol",
    source: "tour-waypoints",
    layout: { "text-field": ["get", "name"], "text-font": FONT, "text-size": 11, "text-offset": [0, 1.1], "text-anchor": "top", "text-optional": true },
    paint: { "text-color": "#222", "text-halo-color": "#fff", "text-halo-width": 1.5 },
  });
  map.addLayer({
    id: "tour-hover",
    type: "circle",
    source: "tour-hover",
    paint: { "circle-radius": 8, "circle-color": "#ff5717", "circle-stroke-color": "#fff", "circle-stroke-width": 3 },
  });
}

/** Marks the places a tour passes (from GET /v1/tours/detail). */
export async function showWaypoints(waypoints) {
  await ensureDetailLayers();
  map.getSource("tour-waypoints").setData({
    type: "FeatureCollection",
    features: waypoints.map((w) => ({ type: "Feature", properties: { name: w.name }, geometry: { type: "Point", coordinates: w.position } })),
  });
}

/** Moves the hover marker to `position` ([lon, lat]), or hides it (null). Called on every mouse move. */
export function markTourPoint(position) {
  const source = map.getSource("tour-hover");
  if (!source) return;
  source.setData({
    type: "FeatureCollection",
    features: position ? [{ type: "Feature", properties: {}, geometry: { type: "Point", coordinates: position } }] : [],
  });
}

/** Removes the tours and brings the routes back (if `showRoutesAgain`). */
export async function clearTours(showRoutesAgain) {
  await ensureDetailLayers();
  const empty = { type: "FeatureCollection", features: [] };
  for (const source of ["tours", "tour-starts", "tour-place", "tour-waypoints", "tour-hover"]) map.getSource(source).setData(empty);
  for (const layer of BASE_ROUTE_LAYERS) {
    map.setLayoutProperty(layer, "visibility", showRoutesAgain ? "visible" : "none");
  }
}

/** Shows or hides the pre-built routes. */
export async function showRoutes(visible) {
  await mapReady;
  for (const layer of [...ROUTE_LAYERS, "route-refs", "route-selected"]) {
    map.setLayoutProperty(layer, "visibility", visible ? "visible" : "none");
  }
}

/** Draws one track (points with lat/lon) on top of the map and zooms to it. */
export async function showTrack(points) {
  await mapReady;
  const line = {
    type: "Feature",
    properties: {},
    geometry: { type: "LineString", coordinates: points.map((p) => [p.lon, p.lat]) },
  };
  const source = map.getSource("selected-track");
  if (source) {
    source.setData(line);
  } else {
    map.addSource("selected-track", { type: "geojson", data: line });
    map.addLayer({
      id: "selected-track-casing",
      type: "line",
      source: "selected-track",
      layout: { "line-cap": "round", "line-join": "round" },
      paint: { "line-color": "#ffffff", "line-width": 7 },
    });
    map.addLayer({
      id: "selected-track",
      type: "line",
      source: "selected-track",
      layout: { "line-cap": "round", "line-join": "round" },
      paint: { "line-color": "#ff5717", "line-width": 4 },
    });
  }
  const lons = points.map((p) => p.lon);
  const lats = points.map((p) => p.lat);
  map.fitBounds(
    [[Math.min(...lons), Math.min(...lats)], [Math.max(...lons), Math.max(...lats)]],
    { padding: 60, maxZoom: 15, duration: 800 },
  );
}
