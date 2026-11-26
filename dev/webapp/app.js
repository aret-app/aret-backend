// Aret test console: service status, Kratos accounts (browser flows) and API calls.
import { showTrack } from "./map.js";

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
