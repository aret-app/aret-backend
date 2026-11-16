// Map of the pilot region with Aret's own vector tiles (OpenMapTiles schema) from Cloudflare Pages.
// A first, simple outdoor style; the real style will live in aret-maps.
import {
  Map as MapLibreMap,
  NavigationControl,
  ScaleControl,
} from "https://cdn.jsdelivr.net/npm/maplibre-gl@6.18.0/dist/maplibre-gl.mjs";

const PILOT_BOUNDS = [9.29, 43.79, 11.38, 44.8]; // aret-maps/config/pilot-region.env
const FONT = ["Noto Sans Regular"];
const NAME = ["coalesce", ["get", "name:it"], ["get", "name"]];

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
