// Dependency-free client for the Zicade control panel. Every input carries a
// data-field with its dotted config path; collect() rebuilds the exact JSON
// schema the backend validates. Mutations send the token in a custom header
// when auth is on; that requirement is the CSRF defense (a cross-site request
// cannot set arbitrary headers). Live status + throughput arrive over the
// /events/metrics SSE stream (no polling).
const $ = (sel) => document.querySelector(sel);
const fieldEl = (path) => document.querySelector(`[data-field="${path}"]`);
const token = () => $("#token").value.trim();

function getStr(path) { const e = fieldEl(path); return e ? e.value.trim() : ""; }
function getBool(path) { const e = fieldEl(path); return !!(e && e.checked); }
function getPort(path) {
  const n = parseInt(getStr(path), 10);
  return Number.isFinite(n) ? n : 0;
}

function setStr(path, v) {
  const e = fieldEl(path);
  if (!e) return;
  v = v == null ? "" : String(v);
  if (e.tagName === "SELECT" && v && ![...e.options].some((o) => o.value === v)) {
    e.add(new Option(v, v));
  }
  e.value = v;
}
function setBool(path, v) { const e = fieldEl(path); if (e) e.checked = !!v; }

function toggleSections() {
  const mode = getStr("routing.mode");
  $(".section-upstream").hidden = mode !== "upstream";
  $(".section-pac").hidden = mode !== "pac";
  // The corp-network gate applies to pac + upstream (it falls back to direct).
  $(".section-corp").hidden = mode !== "pac" && mode !== "upstream";
}

// Parse the DNS-suffix textarea (newline- or comma-separated) into a trimmed,
// non-empty array.
function getSuffixes() {
  return getStr("corpNetwork.dnsSuffixes")
    .split(/[\n,]/)
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
}

// Build the config JSON from the form. Optional sections/fields are only
// included when relevant, matching the backend's skip_serializing_if.
function collect() {
  const mode = getStr("routing.mode");
  const cfg = {
    listen: { host: getStr("listen.host"), port: getPort("listen.port") },
    routing: { mode },
    logging: { level: getStr("logging.level"), format: getStr("logging.format") },
    web: { authRequired: getBool("web.authRequired") },
  };
  if (mode === "upstream") {
    const auth = { mode: getStr("upstream.auth.mode"),
                   package: getStr("upstream.auth.package") };
    const u = getStr("upstream.auth.username");
    const p = getStr("upstream.auth.password");
    if (u) auth.username = u;
    if (p) auth.password = p;
    cfg.routing.upstream = {
      host: getStr("upstream.host"), port: getPort("upstream.port"), auth,
    };
  }
  if (mode === "pac") {
    const pac = { source: getStr("pac.source"),
                  failPolicy: getStr("pac.failPolicy"),
                  auth: { mode: getStr("pac.auth.mode") } };
    const path = getStr("pac.path");
    const url = getStr("pac.url");
    if (path) pac.path = path;
    if (url) pac.url = url;
    cfg.routing.pac = pac;
  }
  // The corp-network gate applies to pac + upstream; only sent for those modes.
  if (mode === "pac" || mode === "upstream") {
    const enabled = getBool("corpNetwork.enabled");
    const poll = parseInt(getStr("corpNetwork.pollSeconds"), 10);
    cfg.routing.corpNetwork = {
      enabled,
      dnsSuffixes: getSuffixes(),
      pollSeconds: Number.isFinite(poll) && poll > 0 ? poll : 30,
    };
  }
  return cfg;
}

function populate(cfg) {
  setStr("listen.host", cfg.listen?.host ?? "127.0.0.1");
  setStr("listen.port", cfg.listen?.port ?? 3129);
  setStr("routing.mode", cfg.routing?.mode ?? "direct");
  const up = cfg.routing?.upstream ?? {};
  setStr("upstream.host", up.host ?? "");
  setStr("upstream.port", up.port ?? "");
  setStr("upstream.auth.mode", up.auth?.mode ?? "none");
  setStr("upstream.auth.package", up.auth?.package ?? "ntlm");
  setStr("upstream.auth.username", up.auth?.username ?? "");
  setStr("upstream.auth.password", up.auth?.password ?? "");
  const pac = cfg.routing?.pac ?? {};
  setStr("pac.source", pac.source ?? "auto");
  setStr("pac.path", pac.path ?? "");
  setStr("pac.url", pac.url ?? "");
  setStr("pac.failPolicy", pac.failPolicy ?? "error");
  setStr("pac.auth.mode", pac.auth?.mode ?? "none");
  const corp = cfg.routing?.corpNetwork ?? {};
  setBool("corpNetwork.enabled", corp.enabled ?? false);
  setStr("corpNetwork.dnsSuffixes", (corp.dnsSuffixes ?? []).join("\n"));
  setStr("corpNetwork.pollSeconds", corp.pollSeconds ?? 30);
  setStr("logging.level", cfg.logging?.level ?? "info");
  setStr("logging.format", cfg.logging?.format ?? "json");
  setBool("web.authRequired", cfg.web?.authRequired ?? false);
  toggleSections();
}

function msg(text, ok) {
  const el = $("#cfg-msg");
  el.textContent = text;
  el.className = ok ? "ok" : "err";
}

async function loadConfig() {
  try {
    const r = await fetch("/api/config");
    populate(await r.json());
    msg("Loaded current configuration.", true);
  } catch (e) { msg("Failed to load config: " + e, false); }
}

async function saveConfig(ev) {
  ev.preventDefault();
  const headers = { "Content-Type": "application/json" };
  const t = token();
  if (t) headers["X-Zicade-Token"] = t;
  const r = await fetch("/api/config", {
    method: "PUT", headers, body: JSON.stringify(collect()),
  });
  if (r.ok) { msg("Configuration saved.", true); }
  else {
    let detail = r.status;
    try { detail = (await r.json()).error || detail; } catch (_) {}
    msg("Error: " + detail, false);
  }
}

// --- Live status + throughput (SSE /events/metrics) ---------------------------
const fmtInt = (n) => (typeof n === "number" ? n : 0).toLocaleString();
function fmtRate(bps) {
  const units = ["B/s", "KB/s", "MB/s", "GB/s"];
  let v = bps, i = 0;
  while (v >= 1024 && i < units.length - 1) { v /= 1024; i++; }
  return (i === 0 ? v.toFixed(0) : v.toFixed(v < 10 ? 1 : 0)) + " " + units[i];
}
function setOnline(ok) {
  const d = $("#status-dot");
  d.className = "dot " + (ok ? "ok" : "down");
  d.title = ok ? "proxy reachable" : "status stream disconnected";
}

const MAXPTS = 60;
const ratesIn = [];
const ratesOut = [];
let prev = null; // { t, bytesIn, bytesOut }

function pushRate(arr, v) { arr.push(v); if (arr.length > MAXPTS) arr.shift(); }
function polyPoints(arr, max) {
  if (arr.length < 2) return "";
  const W = 300, H = 64, pad = 3, step = W / (MAXPTS - 1);
  return arr.map((v, i) => {
    const x = (i * step).toFixed(1);
    const y = (H - pad - (max > 0 ? v / max : 0) * (H - 2 * pad)).toFixed(1);
    return `${x},${y}`;
  }).join(" ");
}
function drawSpark() {
  const max = Math.max(1, ...ratesIn, ...ratesOut);
  $("#spark-in").setAttribute("points", polyPoints(ratesIn, max));
  $("#spark-out").setAttribute("points", polyPoints(ratesOut, max));
}

function applySnapshot(s) {
  $("#st-mode").textContent = s.routing_mode || "—";
  // on-corp is present only when the corp-network gate is active.
  const corpBox = $("#st-corp-box");
  if (typeof s.onCorp === "boolean") {
    corpBox.hidden = false;
    $("#st-corp").textContent = s.onCorp ? "yes" : "no";
  } else {
    corpBox.hidden = true;
  }
  $("#st-listen").textContent = s.listen_addr || "—";
  $("#st-total").textContent = fmtInt(s.requests_total);
  const f = $("#st-failed");
  f.textContent = fmtInt(s.requests_failed);
  f.classList.toggle("bad", (s.requests_failed || 0) > 0);
  $("#st-active").textContent = fmtInt(s.active_connections);

  const now = Date.now();
  const bin = s.bytes_in || 0, bout = s.bytes_out || 0;
  if (prev) {
    const dt = (now - prev.t) / 1000;
    if (dt > 0) {
      // Clamp to >=0 so a counter reset (server restart) doesn't show negative.
      const ri = Math.max(0, (bin - prev.bytesIn) / dt);
      const ro = Math.max(0, (bout - prev.bytesOut) / dt);
      $("#st-in").textContent = fmtRate(ri);
      $("#st-out").textContent = fmtRate(ro);
      pushRate(ratesIn, ri);
      pushRate(ratesOut, ro);
      drawSpark();
    }
  }
  prev = { t: now, bytesIn: bin, bytesOut: bout };
  $("#st-updated").textContent = "updated " + new Date().toLocaleTimeString();
}

function startMetrics() {
  const es = new EventSource("/events/metrics");
  es.onmessage = (e) => {
    try { applySnapshot(JSON.parse(e.data)); setOnline(true); } catch (_) {}
  };
  es.onerror = () => setOnline(false);
}

function startLogs() {
  const box = $("#logs");
  const es = new EventSource("/events/logs");
  es.onmessage = (e) => {
    let line = e.data, level = "";
    try {
      const ev = JSON.parse(e.data);
      level = ev.level || "";
      line = `[${ev.level}] ${ev.target}: ${ev.message}`;
    } catch (_) {}
    const div = document.createElement("div");
    div.textContent = line;
    if (level) div.className = "lvl-" + level;
    box.appendChild(div);
    box.scrollTop = box.scrollHeight;
  };
}

$("#routing-mode").addEventListener("change", toggleSections);
$("#load").addEventListener("click", loadConfig);
$("#config-form").addEventListener("submit", saveConfig);
loadConfig();
startLogs();
startMetrics();
