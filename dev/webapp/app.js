// Aret test console: service status, Kratos accounts (browser flows) and API calls.
import { TOUR_COLORS, clearTours, markTourPoint, showRoutes, showTours, showTrack, showWaypoints } from "./map.js";

// Everything goes through the gateway (dev/gateway/Caddyfile):
//   /kratos/*  → Ory Kratos public API      /api/<service>/*  → our Rust services
//
// Served by the gateway itself (http://127.0.0.1:8080) → same origin, relative URLs.
// Opened from elsewhere, e.g. VS Code Live Server on http://127.0.0.1:5500 → call the gateway
// directly; it allows that origin with cookies (CORS rule in the Caddyfile).
const GATEWAY = location.port === "8080" ? "" : "http://127.0.0.1:8080";
const KRATOS = `${GATEWAY}/kratos`;

const $ = (selector) => document.querySelector(selector);

// Verification flow started by registration (Kratos sends the code right away).
let verificationFlowId = null;

// Session JWT for our services (Kratos tokenizer template "aret"), sent as a Bearer token.
let sessionJwt = null;

// ---------------------------------------------------------------- HTTP + request log

/**
 * Calls the backend with cookies, logs it, and never throws on HTTP errors. Objects are sent as
 * JSON; a string body (e.g. a GPX file) is sent as-is with the Content-Type from `extraHeaders`.
 */
async function call(method, url, body, extraHeaders = {}) {
  const headers = { Accept: "application/json", ...extraHeaders };
  const raw = typeof body === "string";
  if (body !== undefined && !raw) headers["Content-Type"] = "application/json";
  let response;
  try {
    response = await fetch(url, {
      method,
      headers,
      credentials: "include",
      body: body === undefined ? undefined : raw ? body : JSON.stringify(body),
    });
  } catch (error) {
    logRequest(method, url, "network error");
    return { ok: false, status: 0, data: { error: { message: String(error) } } };
  }
  const text = await response.text();
  let data = null;
  try {
    data = text ? JSON.parse(text) : null;
  } catch {
    data = text;
  }
  logRequest(method, url, response.status);
  return { ok: response.ok, status: response.status, data };
}

function logRequest(method, url, status) {
  const item = document.createElement("li");
  const good = typeof status === "number" && status < 400;
  item.className = good ? "ok" : "bad";
  item.textContent = `${new Date().toLocaleTimeString()}  ${method} ${url} → ${status}`;
  $("#log").prepend(item);
}

$("#log-clear").addEventListener("click", () => ($("#log").textContent = ""));

// ---------------------------------------------------------------- service status

const SERVICES = [
  { name: "Kratos (accounts)", url: `${KRATOS}/health/ready` },
  { name: "aret-identity", url: `${GATEWAY}/api/identity/health` },
  { name: "aret-tracks", url: `${GATEWAY}/api/tracks/health` },
];

async function refreshStatus() {
  const list = $("#status-list");
  list.textContent = "";
  for (const service of SERVICES) {
    const item = document.createElement("li");
    item.innerHTML = `<span class="dot wait"></span><span class="detail">checking…</span>`;
    item.querySelector(".dot").after(service.name);
    list.append(item);

    const result = await call("GET", service.url);
    const detail = result.data?.version ? `v${result.data.version}` : result.status || "down";
    item.querySelector(".dot").className = `dot ${result.ok ? "ok" : "bad"}`;
    item.querySelector(".detail").textContent = String(detail);
  }
}

$("#status-refresh").addEventListener("click", refreshStatus);

// ---------------------------------------------------------------- Kratos helpers

/** The CSRF token Kratos expects back with every browser-flow submission. */
function csrfToken(flow) {
  const node = flow?.ui?.nodes?.find((n) => n.attributes?.name === "csrf_token");
  return node?.attributes?.value;
}

/** Human-readable messages from a Kratos flow or error response. */
function kratosMessages(data) {
  if (!data) return "No response.";
  if (data.error) return data.error.reason || data.error.message || "Error.";
  const messages = [
    ...(data.ui?.messages ?? []),
    ...(data.ui?.nodes ?? []).flatMap((node) => node.messages ?? []),
  ];
  return messages.map((m) => m.text).join(" ") || "Request failed.";
}

function showMessage(text, kind = "") {
  const element = $("#account-message");
  element.textContent = text;
  element.className = `message ${kind}`;
}

/** Disables a form's buttons while `task` runs. */
async function busy(form, task) {
  const buttons = form.querySelectorAll("button");
  buttons.forEach((b) => (b.disabled = true));
  try {
    await task();
  } finally {
    buttons.forEach((b) => (b.disabled = false));
  }
}

// ---------------------------------------------------------------- session

async function refreshSession() {
  const result = await call("GET", `${KRATOS}/sessions/whoami`);
  const signedIn = result.ok && result.data?.active;
  $("#signed-out").hidden = signedIn;
  $("#signed-in").hidden = !signedIn;
  if (!signedIn) return;

  const identity = result.data.identity;
  const address = identity.verifiable_addresses?.[0];
  $("#session-email").textContent = identity.traits?.email ?? "—";
  $("#session-verified").textContent = address?.verified ? "yes ✓" : "not yet";
  $("#session-id").textContent = identity.id;
  $("#session-expires").textContent = new Date(result.data.expires_at).toLocaleString();
  $("#verify-form").hidden = Boolean(address?.verified);
  await loadTracks();
}

// ---------------------------------------------------------------- sign up / log in

for (const tab of document.querySelectorAll(".tab")) {
  tab.addEventListener("click", () => {
    document.querySelectorAll(".tab").forEach((t) => t.classList.toggle("active", t === tab));
    $("#register-form").hidden = tab.dataset.tab !== "register";
    $("#login-form").hidden = tab.dataset.tab !== "login";
    $("#recover-panel").hidden = tab.dataset.tab !== "recover";
    showMessage("");
  });
}

$("#register-form").addEventListener("submit", (event) => {
  event.preventDefault();
  const form = event.currentTarget;
  busy(form, async () => {
    const start = await call("GET", `${KRATOS}/self-service/registration/browser`);
    if (!start.ok) return showMessage(kratosMessages(start.data), "bad");

    const fields = new FormData(form);
    const result = await call("POST", `${KRATOS}/self-service/registration?flow=${start.data.id}`, {
      method: "password",
      csrf_token: csrfToken(start.data),
      traits: { email: fields.get("email") },
      password: fields.get("password"),
    });
    if (!result.ok) return showMessage(kratosMessages(result.data), "bad");

    const verification = result.data.continue_with?.find(
      (step) => step.action === "show_verification_ui",
    );
    verificationFlowId = verification?.flow?.id ?? null;
    form.reset();
    showMessage("Account created. A verification code is in Mailpit.", "ok");
    await refreshSession();
  });
});

$("#login-form").addEventListener("submit", (event) => {
  event.preventDefault();
  const form = event.currentTarget;
  busy(form, async () => {
    const start = await call("GET", `${KRATOS}/self-service/login/browser`);
    if (!start.ok) return showMessage(kratosMessages(start.data), "bad");

    const fields = new FormData(form);
    const result = await call("POST", `${KRATOS}/self-service/login?flow=${start.data.id}`, {
      method: "password",
      csrf_token: csrfToken(start.data),
      identifier: fields.get("identifier"),
      password: fields.get("password"),
    });
    if (!result.ok) return showMessage(kratosMessages(result.data), "bad");

    form.reset();
    showMessage("Logged in.", "ok");
    await refreshSession();
  });
});

// ---------------------------------------------------------------- forgot password (recovery)
// Kratos recovery flow with a one-time code (aret-backend#15):
//   1. POST email → 200 state "sent_email" (same answer for unknown addresses)
//   2. POST code  → 200 state "passed_challenge": signed in, and `continue_with` has
//                   `show_settings_ui` with the settings flow (feature flag
//                   use_continue_with_transitions; without it: 422 + redirect_browser_to ?flow=…)
//                   wrong code → 200 with a message; after 5 wrong codes Kratos redirects to a
//                   fresh flow (a new code is needed)
//   3. POST new password to the settings flow → 200 "success"; other sessions are revoked

let recoveryFlow = null;
let settingsFlowId = null;

function showRecoveryStep(step) {
  $("#recover-email-form").hidden = step !== "email";
  $("#recover-code-form").hidden = step !== "code";
  $("#recover-password-form").hidden = step !== "password";
}

function restartRecovery() {
  recoveryFlow = null;
  settingsFlowId = null;
  ["#recover-email-form", "#recover-code-form", "#recover-password-form"].forEach((id) => $(id).reset());
  showRecoveryStep("email");
}

$("#recover-restart").addEventListener("click", () => {
  restartRecovery();
  showMessage("");
});

$("#recover-email-form").addEventListener("submit", (event) => {
  event.preventDefault();
  const form = event.currentTarget;
  busy(form, async () => {
    const start = await call("GET", `${KRATOS}/self-service/recovery/browser`);
    if (!start.ok) return showMessage(kratosMessages(start.data), "bad");

    const result = await call("POST", `${KRATOS}/self-service/recovery?flow=${start.data.id}`, {
      method: "code",
      csrf_token: csrfToken(start.data),
      email: new FormData(form).get("email"),
    });
    if (!result.ok) return showMessage(kratosMessages(result.data), "bad");

    recoveryFlow = result.data;
    showRecoveryStep("code");
    showMessage("If that address has an account, a recovery code is in Mailpit.", "ok");
  });
});

$("#recover-code-form").addEventListener("submit", (event) => {
  event.preventDefault();
  const form = event.currentTarget;
  busy(form, async () => {
    const result = await call("POST", `${KRATOS}/self-service/recovery?flow=${recoveryFlow.id}`, {
      method: "code",
      csrf_token: csrfToken(recoveryFlow),
      code: new FormData(form).get("code").trim(),
    });

    // Success: Kratos signed us in; the next step is the settings flow for a new password
    const settingsStep = result.data?.continue_with?.find((s) => s.action === "show_settings_ui");
    const legacyRedirect = result.status === 422 ? result.data?.redirect_browser_to : null;
    if (settingsStep || legacyRedirect) {
      settingsFlowId = settingsStep?.flow?.id ?? new URL(legacyRedirect).searchParams.get("flow");
      showRecoveryStep("password");
      return showMessage("Code accepted. Choose a new password.", "ok");
    }
    // Too many wrong codes: the browser followed Kratos's redirect to a brand-new flow
    if (result.ok && result.data?.id !== recoveryFlow.id) {
      restartRecovery();
      return showMessage("Too many wrong codes. Request a new one.", "bad");
    }
    showMessage(kratosMessages(result.data), "bad");
  });
});

$("#recover-password-form").addEventListener("submit", (event) => {
  event.preventDefault();
  const form = event.currentTarget;
  busy(form, async () => {
    const flow = await call("GET", `${KRATOS}/self-service/settings/flows?id=${settingsFlowId}`);
    if (!flow.ok) return showMessage(kratosMessages(flow.data), "bad");

    const result = await call("POST", `${KRATOS}/self-service/settings?flow=${settingsFlowId}`, {
      method: "password",
      csrf_token: csrfToken(flow.data),
      password: new FormData(form).get("password"),
    });
    if (!result.ok || result.data?.state !== "success") {
      return showMessage(kratosMessages(result.data), "bad");
    }
    restartRecovery();
    showMessage("Password changed. You're signed in; every other device was signed out.", "ok");
    await refreshSession();
  });
});

// ---------------------------------------------------------------- email verification

/** A verification flow: the one registration started, or a new one that emails a new code. */
async function verificationFlow({ sendNewCode }) {
  if (verificationFlowId && !sendNewCode) {
    const existing = await call("GET", `${KRATOS}/self-service/verification/flows?id=${verificationFlowId}`);
    if (existing.ok) return existing.data;
  }
  const start = await call("GET", `${KRATOS}/self-service/verification/browser`);
  if (!start.ok) throw new Error(kratosMessages(start.data));
  const sent = await call("POST", `${KRATOS}/self-service/verification?flow=${start.data.id}`, {
    method: "code",
    csrf_token: csrfToken(start.data),
    email: $("#session-email").textContent,
  });
  if (!sent.ok) throw new Error(kratosMessages(sent.data));
  verificationFlowId = sent.data.id;
  return sent.data;
}

$("#verify-form").addEventListener("submit", (event) => {
  event.preventDefault();
  const form = event.currentTarget;
  busy(form, async () => {
    try {
      const flow = await verificationFlow({ sendNewCode: false });
      const code = new FormData(form).get("code").trim();
      const result = await call("POST", `${KRATOS}/self-service/verification?flow=${flow.id}`, {
        method: "code",
        csrf_token: csrfToken(flow),
        code,
      });
      if (!result.ok || result.data?.state !== "passed_challenge") {
        return showMessage(kratosMessages(result.data), "bad");
      }
      form.reset();
      showMessage("Email verified.", "ok");
      await refreshSession();
    } catch (error) {
      showMessage(error.message, "bad");
    }
  });
});

$("#resend-code").addEventListener("click", () => {
  busy($("#verify-form"), async () => {
    try {
      await verificationFlow({ sendNewCode: true });
      showMessage("New code sent. Check Mailpit.", "ok");
    } catch (error) {
      showMessage(error.message, "bad");
    }
  });
});

// ---------------------------------------------------------------- log out

$("#logout").addEventListener("click", async () => {
  const start = await call("GET", `${KRATOS}/self-service/logout/browser`);
  if (!start.ok) return showMessage(kratosMessages(start.data), "bad");
  await call("GET", `${KRATOS}/self-service/logout?token=${start.data.logout_token}`);
  verificationFlowId = null;
  setJwt(null);
  showMessage("Logged out.");
  await refreshSession();
});

// ---------------------------------------------------------------- delete account (GDPR)

/**
 * Asks identity-svc to delete the account. Returns the HTTP result. A 403 means the last password
 * entry is older than 15 minutes: the user must confirm their password first.
 */
async function requestDeletion() {
  // Always a new JWT: it carries the login time (`authenticated_at`) identity-svc checks
  const result = await call("GET", `${KRATOS}/sessions/whoami?tokenize_as=aret`);
  const token = result.ok ? result.data?.tokenized : null;
  if (!token) return { ok: false, status: 401, data: { detail: "Log in first." } };
  return call("DELETE", `${GATEWAY}/api/identity/v1/account`, undefined, {
    Authorization: `Bearer ${token}`,
  });
}

async function finishDeletion(deletion) {
  if (deletion.status === 403) {
    $("#reauth-form").hidden = false;
    $("#reauth-form").password.focus();
    return showMessage(
      "For your safety, deleting needs a recent login: you entered your password more than " +
        "15 minutes ago. Confirm it below.",
    );
  }
  $("#reauth-form").hidden = true;
  if (!deletion.ok) {
    return showMessage(deletion.data?.detail ?? `Deletion failed (HTTP ${deletion.status}).`, "bad");
  }
  verificationFlowId = null;
  setJwt(null);
  showMessage("Account deleted. Your tracks are being removed from every service.", "ok");
  await refreshSession();
}

$("#delete-account").addEventListener("click", async () => {
  const sure = confirm("Delete your Aret account and all your tracks? This can't be undone.");
  if (!sure) return;
  await finishDeletion(await requestDeletion());
});

// Re-authentication: a Kratos login flow with `refresh=true` checks the password again for the
// current session and updates its login time, without logging out.
$("#reauth-form").addEventListener("submit", (event) => {
  event.preventDefault();
  const form = event.currentTarget;
  busy(form, async () => {
    const start = await call("GET", `${KRATOS}/self-service/login/browser?refresh=true`);
    if (!start.ok) return showMessage(kratosMessages(start.data), "bad");
    const result = await call("POST", `${KRATOS}/self-service/login?flow=${start.data.id}`, {
      method: "password",
      csrf_token: csrfToken(start.data),
      identifier: $("#session-email").textContent,
      password: form.password.value,
    });
    form.password.value = "";
    if (!result.ok) return showMessage(kratosMessages(result.data), "bad");
    await finishDeletion(await requestDeletion());
  });
});

$("#reauth-cancel").addEventListener("click", () => {
  $("#reauth-form").hidden = true;
  showMessage("Account not deleted.");
});

// ---------------------------------------------------------------- session JWT + tracks API

/** Decodes a JWT's payload for display only. The services verify the signature, not this page. */
function jwtClaims(token) {
  const payload = token.split(".")[1].replace(/-/g, "+").replace(/_/g, "/");
  return JSON.parse(atob(payload));
}

function setJwt(token) {
  sessionJwt = token;
  $("#jwt-info").hidden = !token;
  if (!token) return;
  const claims = jwtClaims(token);
  $("#jwt-sub").textContent = claims.sub;
  $("#jwt-aud").textContent = [].concat(claims.aud ?? "—").join(", ");
  $("#jwt-iss").textContent = claims.iss;
  $("#jwt-exp").textContent = new Date(claims.exp * 1000).toLocaleTimeString();
  $("#jwt-login").textContent = claims.authenticated_at
    ? new Date(claims.authenticated_at).toLocaleTimeString()
    : "—";
}

$("#get-jwt").addEventListener("click", async () => {
  const result = await call("GET", `${KRATOS}/sessions/whoami?tokenize_as=aret`);
  if (!result.ok || !result.data?.tokenized) {
    setJwt(null);
    $("#api-result").textContent =
      `HTTP ${result.status}: no JWT. Log in first.\n${JSON.stringify(result.data, null, 2)}`;
    return;
  }
  setJwt(result.data.tokenized);
  $("#api-result").textContent = "Got a session JWT. Now call the API.";
});

/** A synthetic 1 km climb near Pania della Croce, starting now; for trying the API. */
function sampleTrack() {
  const start = Date.now();
  const point = (i) => ({
    lat: 44.0 + i * 0.0009,
    lon: 10.3,
    ele: 1000 + i * 2,
    time: new Date(start + i * 60_000).toISOString(),
  });
  return {
    id: crypto.randomUUID(),
    title: `Sample hike ${new Date(start).toLocaleTimeString()}`,
    points: Array.from({ length: 11 }, (_, i) => point(i)),
  };
}

$("#save-sample").addEventListener("click", async () => {
  if (!sessionJwt) {
    $("#api-result").textContent = "Get a JWT first (step 1).";
    return;
  }
  const result = await call("POST", `${GATEWAY}/api/tracks/v1/tracks`, sampleTrack(), {
    Authorization: `Bearer ${sessionJwt}`,
  });
  $("#api-result").textContent = `HTTP ${result.status}\n${JSON.stringify(result.data, null, 2)}`;
});

$("#call-tracks").addEventListener("click", async () => {
  const headers = sessionJwt ? { Authorization: `Bearer ${sessionJwt}` } : {};
  const result = await call("GET", `${GATEWAY}/api/tracks/v1/tracks`, undefined, headers);
  const sent = sessionJwt ? "with JWT" : "without JWT";
  $("#api-result").textContent =
    `HTTP ${result.status} (${sent})\n${JSON.stringify(result.data, null, 2)}`;
});

// ---------------------------------------------------------------- my tracks (GPX)

const TRACKS_API = `${GATEWAY}/api/tracks/v1/tracks`;
let selectedTrackId = null;

/** A session JWT valid for at least 30 more seconds, fetched from Kratos if needed. */
async function ensureJwt() {
  const expiresSoon = sessionJwt && jwtClaims(sessionJwt).exp * 1000 < Date.now() + 30_000;
  if (!sessionJwt || expiresSoon) {
    const result = await call("GET", `${KRATOS}/sessions/whoami?tokenize_as=aret`);
    setJwt(result.ok ? result.data?.tokenized ?? null : null);
  }
  return sessionJwt;
}

function tracksMessage(text, kind = "") {
  $("#tracks-message").textContent = text;
  $("#tracks-message").className = `message ${kind}`;
}

async function loadTracks() {
  const jwt = await ensureJwt();
  if (!jwt) return tracksMessage("Log in to see your tracks.", "bad");
  const result = await call("GET", `${TRACKS_API}?limit=50`, undefined, {
    Authorization: `Bearer ${jwt}`,
  });
  if (!result.ok) return tracksMessage(kratosMessages(result.data), "bad");

  const body = $("#tracks-body");
  body.textContent = "";
  for (const track of result.data.items) {
    const row = document.createElement("tr");
    row.dataset.id = track.id;
    row.classList.toggle("selected", track.id === selectedTrackId);
    const cells = [
      track.title || "(untitled)",
      new Date(track.started_at).toLocaleDateString(),
      (track.distance_m / 1000).toFixed(2),
      Math.round(track.ascent_m),
    ];
    for (const value of cells) {
      const cell = document.createElement("td");
      cell.textContent = value; // textContent: titles are user input, never HTML
      row.append(cell);
    }
    row.addEventListener("click", () => selectTrack(track));
    body.append(row);
  }
  tracksMessage(result.data.items.length ? "" : "No tracks yet. Upload a GPX file.");
}

async function selectTrack(track) {
  const jwt = await ensureJwt();
  if (!jwt) return tracksMessage("Log in first.", "bad");
  const result = await call("GET", `${TRACKS_API}/${track.id}`, undefined, {
    Authorization: `Bearer ${jwt}`,
  });
  if (!result.ok) return tracksMessage(kratosMessages(result.data), "bad");

  selectedTrackId = track.id;
  document.querySelectorAll("#tracks-body tr").forEach((row) => {
    row.classList.toggle("selected", row.dataset.id === track.id);
  });
  $("#track-actions").hidden = false;
  $("#track-selected").textContent =
    `${track.title || "(untitled)"} · ${result.data.points.length} points`;
  await showTrack(result.data.points);
  $("#map").scrollIntoView({ behavior: "smooth", block: "center" });
}

$("#tracks-refresh").addEventListener("click", loadTracks);

$("#gpx-form").addEventListener("submit", (event) => {
  event.preventDefault();
  const form = event.currentTarget;
  busy(form, async () => {
    const file = $("#gpx-file").files[0];
    if (!file) return tracksMessage("Choose a .gpx file first.", "bad");
    const jwt = await ensureJwt();
    if (!jwt) return tracksMessage("Log in first.", "bad");

    // A new id per upload; the app would keep it so a retried upload never duplicates
    const result = await call("POST", `${TRACKS_API}/gpx?id=${crypto.randomUUID()}`,
      await file.text(), {
        Authorization: `Bearer ${jwt}`,
        "Content-Type": "application/gpx+xml",
      });
    if (!result.ok) return tracksMessage(kratosMessages(result.data), "bad");

    form.reset();
    tracksMessage(`Imported "${result.data.title || "(untitled)"}".`, "ok");
    await loadTracks();
    await selectTrack(result.data);
  });
});

$("#track-download").addEventListener("click", async () => {
  const jwt = await ensureJwt();
  if (!jwt || !selectedTrackId) return;
  const url = `${TRACKS_API}/${selectedTrackId}/gpx`;
  const response = await fetch(url, { headers: { Authorization: `Bearer ${jwt}` } });
  logRequest("GET", url, response.status);
  if (!response.ok) return tracksMessage(`Download failed (${response.status}).`, "bad");

  const name = /filename="([^"]+)"/.exec(response.headers.get("Content-Disposition") ?? "");
  const link = document.createElement("a");
  link.href = URL.createObjectURL(await response.blob());
  link.download = name?.[1] ?? `aret-${selectedTrackId}.gpx`;
  link.click();
  URL.revokeObjectURL(link.href);
});

// ---------------------------------------------------------------- start

refreshStatus();
refreshSession();

// ---------------------------------------------------------------- pre-built routes on the map

$("#show-routes").addEventListener("change", (event) => {
  showRoutes(event.currentTarget.checked);
  $("#routes-legend").hidden = !event.currentTarget.checked;
});

// ---------------------------------------------------------------- tours (places-svc)

const PLACES = `${GATEWAY}/api/places`;
const escapeHtml = (text) => String(text ?? "").replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`);
const hoursMinutes = (minutes) =>
  minutes < 60 ? `${minutes} min` : `${Math.floor(minutes / 60)} h ${String(minutes % 60).padStart(2, "0")}`;
// "CAI 9 → CAI 125 → CAI 126" reads better as "CAI 9 → 125 → 126"
const ways = (via) =>
  via.map((name, i) => (i > 0 && name.startsWith("CAI ") && via[i - 1].startsWith("CAI ") ? name.slice(4) : name)).join(" → ");
let searchTimer = null;
let shown = null; // { place, tours }

function searchMessage(text, kind = "") {
  const element = $("#search-message");
  element.textContent = text;
  element.className = `message ${kind}`;
}

$("#place-search").addEventListener("input", (event) => {
  clearTimeout(searchTimer);
  const query = event.currentTarget.value.trim();
  if (query.length < 2) {
    $("#place-results").replaceChildren();
    return;
  }
  // Wait until typing pauses, so one search runs per word, not per key
  searchTimer = setTimeout(() => findPlaces(query), 250);
});

async function findPlaces(query) {
  const result = await call("GET", `${PLACES}/v1/places?q=${encodeURIComponent(query)}`);
  const list = $("#place-results");
  list.replaceChildren();
  if (!result.ok) return searchMessage(result.data?.detail ?? `Search failed (HTTP ${result.status}).`, "bad");
  searchMessage(result.data.places.length ? "" : "No place found.");
  for (const place of result.data.places) {
    const item = document.createElement("li");
    const ele = place.ele != null ? ` · ${Math.round(place.ele)} m` : "";
    item.innerHTML = `<button type="button" class="ghost"><strong>${escapeHtml(place.name)}</strong> <small>${escapeHtml(place.type)}${ele}</small></button>`;
    item.querySelector("button").addEventListener("click", () => findTours(place));
    list.append(item);
  }
}

async function findTours(place) {
  $("#place-results").replaceChildren();
  $("#place-search").value = place.name;
  searchMessage("Finding tours…");
  const result = await call("GET", `${PLACES}/v1/tours?to=${encodeURIComponent(place.id)}`);
  const list = $("#route-results");
  list.replaceChildren();
  if (!result.ok) return searchMessage(result.data?.detail ?? `Search failed (HTTP ${result.status}).`, "bad");
  const tours = result.data.tours;
  shown = { place: result.data.place, tours };
  $("#tour-detail").hidden = true;
  showWaypoints([]);
  $("#tours-clear").hidden = false;
  searchMessage(
    tours.length
      ? `${tours.length} tour${tours.length === 1 ? "" : "s"} to ${place.name}, quickest first. ` +
          `Data © OpenStreetMap contributors (ODbL), ${result.data.data.osm_data_at?.slice(0, 10) ?? ""}.`
      : `No tour to ${place.name} from a car park or village on the marked trails.`,
  );
  tours.forEach((tour, index) => {
    const item = document.createElement("li");
    item.className = "tour";
    const shape = tour.shape === "loop" ? "Loop" : "Out &amp; back";
    const down = tour.shape === "loop" ? `<br><small>down: ${escapeHtml(ways(tour.via_down))}</small>` : "";
    item.innerHTML =
      `<button type="button" class="tour-card">` +
      `<span class="tour-number" style="background:${TOUR_COLORS[index % TOUR_COLORS.length]}">${index + 1}</span>` +
      `<span><strong>${escapeHtml(tour.title)}</strong> <span class="badge">${shape}</span><br>` +
      `⏱ ${hoursMinutes(tour.duration_min)} · ${(tour.length_m / 1000).toFixed(1)} km · ↗ ${Math.round(tour.ascent_m)} m · ` +
      `${escapeHtml(tour.difficulty)} · start ${Math.round(tour.start.ele)} m<br>` +
      `<small>up: ${escapeHtml(ways(tour.via_up))}</small>${down}</span></button>`;
    item.querySelector("button").addEventListener("click", () => selectTour(index));
    list.append(item);
  });
  // The difficulty legend doesn't apply while only the tours are on the map
  $("#routes-legend").hidden = true;
  if (tours.length) showTours(shown.place, tours);
}

async function selectTour(index) {
  if (!shown) return;
  document.querySelectorAll("#route-results .tour").forEach((item, i) => item.classList.toggle("selected", i === index));
  showTours(shown.place, shown.tours, index);
  const tour = shown.tours[index];
  const result = await call(
    "GET",
    `${PLACES}/v1/tours/detail?to=${encodeURIComponent(shown.place.id)}&from=${encodeURIComponent(tour.start.id)}`,
  );
  if (!result.ok) return searchMessage(result.data?.detail ?? `Tour detail failed (HTTP ${result.status}).`, "bad");
  showTourDetail(result.data.tour);
}

// ---------------------------------------------------------------- tour detail card

const WAYPOINT_ICONS = { peak: "▲", pass: "⩘", hut: "⌂", water: "💧", settlement: "●" };
const km = (metres) => `${(metres / 1000).toFixed(1)} km`;

function showTourDetail(tour) {
  $("#tour-detail").hidden = false;
  $("#tour-detail-title").textContent = tour.title;
  $("#tour-gpx").href = `${PLACES}/${tour.gpx}`;
  $("#tour-stats").textContent =
    `⏱ ${hoursMinutes(tour.duration_min)} · ${km(tour.length_m)} · ↗ ${tour.ascent_m} m · ↘ ${tour.descent_m} m · ` +
    `highest ${tour.highest?.ele ?? tour.ele_max} m · lowest ${tour.lowest?.ele ?? "?"} m · ${tour.difficulty} · ` +
    `${tour.shape === "loop" ? "loop" : "out & back"}`;

  const slopes = [
    ...tour.climbs.map((s) => ({ ...s, kind: "↗ climb" })),
    ...tour.descents.map((s) => ({ ...s, kind: "↘ descent" })),
  ].sort((a, b) => a.from_m - b.from_m);
  $("#tour-slopes tbody").replaceChildren(
    ...slopes.map((s) => {
      const row = document.createElement("tr");
      row.innerHTML =
        `<td>${s.kind}</td><td>${km(s.from_m)}</td><td>${km(s.to_m - s.from_m)}</td><td>${s.change_m} m</td>` +
        `<td>${s.gradient_pct}%</td><td>${s.max_gradient_pct}%</td>`;
      row.addEventListener("mouseenter", () => elevationChart.highlight(s.from_m, s.to_m));
      row.addEventListener("mouseleave", () => elevationChart.highlight(null));
      return row;
    }),
  );
  $("#tour-waypoints").replaceChildren(
    ...tour.waypoints.map((w) => {
      const item = document.createElement("li");
      const ele = w.ele != null ? ` · ${w.ele} m` : "";
      item.innerHTML = `${WAYPOINT_ICONS[w.kind] ?? "•"} <strong>${escapeHtml(w.name)}</strong> <small>${km(w.distance_m)}${ele}</small>`;
      item.addEventListener("mouseenter", () => {
        markTourPoint(w.position);
        elevationChart.cursorAt(w.distance_m);
      });
      item.addEventListener("mouseleave", () => {
        markTourPoint(null);
        elevationChart.cursorAt(null);
      });
      return item;
    }),
  );
  showWaypoints(tour.waypoints);
  elevationChart.draw(tour.profile.samples, tour.waypoints, tour.difficulties);
  // Legend: the grades on this tour, easiest first, with how far each is walked
  const walked = {};
  for (const g of tour.difficulties) walked[g.difficulty] = (walked[g.difficulty] ?? 0) + g.to_m - g.from_m;
  $("#elevation-legend").innerHTML = Object.keys(GRADE_COLORS)
    .filter((grade) => walked[grade] != null || (grade === "EEA" && Object.keys(walked).some((g) => g.startsWith("EEA"))))
    .map((grade) => {
      const metres = grade === "EEA"
        ? Object.entries(walked).filter(([g]) => g.startsWith("EEA")).reduce((sum, [, m]) => sum + m, 0)
        : walked[grade];
      return `<span style="--c:${GRADE_COLORS[grade]}">${grade} ${km(metres)}</span>`;
    })
    .join("");
}

// Elevation chart colours: a ramp from easy (green) to hard (purple); grey = not recorded in OSM.
// The map keeps each tour in its own colour.
const GRADE_COLORS = { T: "#2e9d4b", E: "#d4b106", EE: "#e8590c", EEA: "#8e44ad", unknown: "#8a8a8a" };
const gradeColor = (grade) => GRADE_COLORS[grade?.startsWith("EEA") ? "EEA" : grade] ?? GRADE_COLORS.unknown;

/**
 * The elevation profile as SVG: distance across, elevation up. Moving over it shows a cursor with
 * distance and elevation and moves the marker on the map to that sample's position.
 */
const elevationChart = (() => {
  const svg = $("#elevation-chart");
  const NS = "http://www.w3.org/2000/svg";
  const PAD = { left: 48, right: 12, top: 12, bottom: 24 };
  let samples = [];
  let stretches = [];
  let scale = null;
  let layer = null; // the cursor and highlight, redrawn on hover

  const element = (name, attributes, parent = svg) => {
    const node = document.createElementNS(NS, name);
    for (const [key, value] of Object.entries(attributes)) node.setAttribute(key, value);
    parent.append(node);
    return node;
  };
  // Round tick steps: 1, 2, 5 × 10^n
  const step = (range, ticks) => {
    const raw = range / ticks;
    const power = 10 ** Math.floor(Math.log10(raw));
    return [1, 2, 5, 10].map((m) => m * power).find((s) => s >= raw);
  };

  let drawn = null; // what draw() was given, to redraw at a new width
  // The grade at distance `d`
  const gradeAt = (d) => (stretches.find((g) => d >= g.from_m && d <= g.to_m) ?? stretches.at(-1))?.difficulty ?? "unknown";

  function draw(newSamples, waypoints, difficulties) {
    drawn = [newSamples, waypoints, difficulties];
    samples = newSamples;
    stretches = difficulties;
    svg.replaceChildren();
    if (samples.length < 2) return;
    const width = svg.clientWidth || 800;
    const height = svg.clientHeight || 200;
    svg.setAttribute("viewBox", `0 0 ${width} ${height}`);
    const length = samples.at(-1)[0];
    const elevations = samples.map((s) => s[1]);
    const yStep = step(Math.max(...elevations) - Math.min(...elevations) || 100, 4);
    const low = Math.floor(Math.min(...elevations) / yStep) * yStep;
    const high = Math.ceil(Math.max(...elevations) / yStep) * yStep;
    const x = (d) => PAD.left + (d / length) * (width - PAD.left - PAD.right);
    const y = (e) => height - PAD.bottom - ((e - low) / (high - low || 1)) * (height - PAD.top - PAD.bottom);
    scale = { x, y, length, width, height };

    for (let e = low; e <= high; e += yStep) {
      element("line", { class: "grid", x1: PAD.left, x2: width - PAD.right, y1: y(e), y2: y(e) });
      element("text", { x: PAD.left - 6, y: y(e) + 4, "text-anchor": "end" }).textContent = `${e} m`;
    }
    const xStep = step(length / 1000, 8) * 1000;
    for (let d = 0; d <= length; d += xStep) {
      element("text", { x: x(d), y: height - 6, "text-anchor": "middle" }).textContent = `${d / 1000} km`;
    }
    // Runs of samples with the same grade (taken in the middle of each 50 m step), each in its colour
    const runs = [];
    for (let i = 0; i + 1 < samples.length; i++) {
      const grade = gradeAt((samples[i][0] + samples[i + 1][0]) / 2);
      const run = runs.at(-1);
      if (run && run.grade === grade) run.last = i + 1;
      else runs.push({ grade, first: i, last: i + 1 });
    }
    for (const { grade, first, last } of runs) {
      const part = samples.slice(first, last + 1);
      const line = part.map((s, i) => `${i ? "L" : "M"}${x(s[0]).toFixed(1)},${y(s[1]).toFixed(1)}`).join("");
      const color = gradeColor(grade);
      element("path", { d: `${line}L${x(part.at(-1)[0])},${y(low)}L${x(part[0][0])},${y(low)}Z`, fill: color, "fill-opacity": 0.25 });
      element("path", { d: line, fill: "none", stroke: color, "stroke-width": 2.5, "stroke-linejoin": "round", "stroke-linecap": "round" });
    }
    element("line", { class: "axis", x1: PAD.left, x2: width - PAD.right, y1: y(low), y2: y(low) });
    for (const w of waypoints) {
      element("line", { class: "waypoint-tick", x1: x(w.distance_m), x2: x(w.distance_m), y1: y(low), y2: y(low) + 5 });
    }
    layer = element("g", {});
    element("rect", { x: 0, y: 0, width, height, fill: "transparent" });
  }

  // The sample nearest to distance `d`: samples are evenly spaced, so it's a division
  const nearest = (d) => samples[Math.max(0, Math.min(samples.length - 1, Math.round((d / scale.length) * (samples.length - 1))))];

  function cursorAt(distance) {
    if (!layer) return;
    layer.replaceChildren();
    if (distance == null) return;
    const [d, ele] = nearest(distance);
    const { x, y, height, width } = scale;
    element("line", { class: "cursor", x1: x(d), x2: x(d), y1: PAD.top, y2: height - PAD.bottom }, layer);
    element("circle", { cx: x(d), cy: y(ele), r: 4, fill: "#ff5717", stroke: "#fff", "stroke-width": 2 }, layer);
    const label = `${(d / 1000).toFixed(2)} km · ${Math.round(ele)} m · ${gradeAt(d)}`;
    const labelX = Math.min(x(d) + 8, width - PAD.right - 160);
    element("rect", { class: "readout-bg", x: labelX - 4, y: PAD.top, width: 160, height: 20, rx: 4 }, layer);
    element("text", { class: "readout", x: labelX + 2, y: PAD.top + 14 }, layer).textContent = label;
  }

  function highlight(from, to) {
    if (!layer) return;
    layer.replaceChildren();
    if (from == null) return;
    const { x, height } = scale;
    element("rect", { x: x(from), y: PAD.top, width: x(to) - x(from), height: height - PAD.top - PAD.bottom, fill: "#ff5717", "fill-opacity": 0.15 }, layer);
  }

  // Pointer events on the whole chart: distance from the pointer's x
  svg.addEventListener("pointermove", (event) => {
    if (!scale || samples.length < 2) return;
    const box = svg.getBoundingClientRect();
    const px = ((event.clientX - box.left) / box.width) * scale.width;
    const d = ((px - PAD.left) / (scale.width - PAD.left - PAD.right)) * scale.length;
    if (d < 0 || d > scale.length) return cursorAt(null), markTourPoint(null);
    cursorAt(d);
    const sample = nearest(d);
    markTourPoint([sample[2], sample[3]]);
  });
  svg.addEventListener("pointerleave", () => {
    cursorAt(null);
    markTourPoint(null);
  });
  // Redraw at the new width when the window changes size
  let last = null;
  window.addEventListener("resize", () => {
    clearTimeout(last);
    last = setTimeout(() => drawn && draw(...drawn), 200);
  });

  return { draw, cursorAt, highlight };
})();

$("#tours-clear").addEventListener("click", () => {
  shown = null;
  $("#route-results").replaceChildren();
  $("#place-search").value = "";
  $("#tours-clear").hidden = true;
  searchMessage("");
  $("#routes-legend").hidden = !$("#show-routes").checked;
  $("#tour-detail").hidden = true;
  clearTours($("#show-routes").checked);
});
