# Zicade (Rust rebuild) — Project Prompt

> **For the AI agent executing this project.** Read the whole document before writing any
> code. This is a **test-driven rebuild** of an existing Zig proxy facade. You are not porting
> line by line — you are rebuilding clean in Rust, carrying forward the *lessons* (encoded as
> tests) rather than the implementation.

---

## 1. Mission

Build **Zicade**: a local HTTP/HTTPS forward-proxy that sits between desktop apps and a
corporate upstream proxy (a McAfee Web Gateway), on a **domain-joined Windows 11** machine.
Zicade authenticates to the upstream on the app's behalf using **Windows SSPI
(Negotiate / NTLM)** so that apps which can't do integrated auth "just work", and it resolves
per-request routing via a **PAC file**. It ships with a **local web UI** (loopback-only) for
configuration and live log streaming.

This replaces a prior Zig 0.16 implementation. That implementation *worked* against the live
corporate proxy, but was hard to maintain (large files, a churning async std API, and a class
of memory-safety bugs). The Rust rebuild exists to eliminate the memory-bug class structurally
and to add a proper configuration/observability UI.

**Non-negotiable domain facts** (the real environment this runs against):

- The corporate upstream proxy is reachable at **`wp8080`** (host:port; treat as configurable).
  It returns **HTTP 407 Proxy Authentication Required** until a valid Negotiate token is
  presented.
- The machine is **domain-joined**; SSPI can acquire a real outbound credential handle for the
  logged-in user with **no stored password**.
- Some hosts must go **DIRECT** (internal) and others **through the upstream** (external) — the
  PAC file decides. There may be a jump host in the environment: **Zicade must talk to `wp8080`
  directly; never route Zicade's own upstream traffic through any jump host.**
- HTTPS traffic uses the **`CONNECT` method** to establish a tunnel; HTTP traffic is
  **forwarded** (absolute-form request line rewritten to origin-form for the upstream as
  appropriate).

---

## 2. Working agreement: strict TDD

This is the core process constraint. Follow it for every feature and every bug-lesson.

1. **Write the failing test first.** For each unit of behavior, write a test that encodes the
   expected behavior and currently fails (red). Commit the red test.
2. **Make it green** with the smallest change that satisfies the test. No speculative code that
   no test demands.
3. **Refactor** with tests green.
4. **`cargo test` is the single source of truth.** It must be runnable at every commit. A
   commit that leaves the default test suite red is not allowed (a *newly added, not-yet-green*
   test is fine only within a red→green working increment and must go green before the feature
   is called done).
5. **One test green at a time.** Do not batch-implement. Turn the backlog of failing tests
   green one by one, in dependency order.
6. **Pre-existing / environment-gated failures are recorded, not hidden.** Live-proxy tests
   (see §7) are gated behind an env var and skipped by default; skipping is explicit and
   logged, never silent.

Start each milestone by writing its full set of failing tests (the "red backlog") from the
acceptance criteria, then drive them green.

---

## 3. Tech stack

- **Language:** Rust (stable, latest). Edition 2021+.
- **Async runtime:** `tokio` (multi-threaded).
- **Proxy data path:** `hyper` (1.x) for HTTP parsing/serving and upstream client; raw
  `tokio::net::TcpStream` bidirectional copy for `CONNECT` tunnels.
- **Local API + web server:** `axum`.
- **Frontend:** server-rendered **htmx** pages (minimal JS), **Server-Sent Events (SSE)** for
  live log streaming. Static assets embedded in the binary (e.g. `rust-embed`).
- **Windows integration:** `windows` (windows-rs) crate for **SSPI** (`AcquireCredentialsHandle`,
  `InitializeSecurityContext`, `AcceptSecurityContext`) and **WinHTTP** PAC resolution
  (`WinHttpGetProxyForUrl` / `WINHTTP_AUTOPROXY_OPTIONS`). This is the *only* place FFI/unsafe
  lives (see §6).
- **Config:** `serde` + JSON on disk; `serde` structs are the schema.
- **Logging/observability:** `tracing` + `tracing-subscriber`, with a custom layer that fans
  structured events out to the SSE log stream (see §5.3).
- **Tests:** built-in `cargo test`; add `wiremock` or a hand-rolled tokio TCP fixture for
  proxy/upstream simulation; keep live-proxy tests behind an env gate.

---

## 4. Architecture (Cargo workspace)

A workspace of small, single-responsibility crates. Keep files to a few hundred lines; when a
file grows past that, split it. This directly addresses the maintainability failure of the
prior build (single files of 1,000–3,700 lines).

```
zicade/
├─ Cargo.toml                      # workspace
├─ crates/
│  ├─ zicade-config/               # config schema, load/save, validation, migration
│  ├─ zicade-routing/              # RoutingMode, RouteSelector, PAC resolver trait
│  ├─ zicade-auth/                 # UpstreamAuth trait; Negotiate handshake state machine
│  ├─ zicade-win/                  # ★ ONLY crate with unsafe/FFI: SSPI + WinHTTP PAC bindings
│  ├─ zicade-proxy/               # the proxy server: accept loop, CONNECT tunnel, HTTP forward
│  ├─ zicade-web/                  # axum local API + htmx UI + SSE log stream
│  └─ zicade-observe/              # tracing setup + in-memory ring buffer + SSE broadcast
└─ apps/
   └─ zicade/                      # the binary: wires config → proxy + web, lifecycle, tray/service
```

Design rules:

- **`zicade-win` is the sole home of `unsafe`/FFI.** Every other crate is safe Rust. All SSPI
  and WinHTTP logic lives here behind safe, testable trait interfaces. Zero duplication of this
  logic anywhere else. On non-Windows targets it compiles to a stub returning
  `UnsupportedPlatform`, so the rest of the workspace builds and unit-tests on any OS.
- **Traits at every seam.** `RouteResolver`, `UpstreamAuthenticator`, `PacBackend`,
  `ClockSource`, etc. are traits so tests can inject fakes and the live Windows implementation
  is one impl among several. This is how we test the state machines without a domain-joined box.
- **Narrow public APIs.** Only what another crate needs is `pub`. Prefer `pub(crate)`.

---

## 5. Feature specification

### 5.1 Proxy core (`zicade-proxy`)

- Listens on a configurable loopback host:port (default `127.0.0.1:3129`).
- **Routing modes:**
  - `direct` — open a socket straight to the origin; no upstream.
  - `upstream` — always go through one configured upstream proxy.
  - `pac` — resolve per-request via the PAC file; result is either `DIRECT` or a specific
    upstream `host:port`.
- **HTTP forwarding:** parse request, apply routing decision, forward to upstream/origin,
  stream response back. Handle chunked and content-length bodies without corruption
  (see the POST-integrity lesson in §8).
- **HTTPS / `CONNECT` tunneling:** on `CONNECT host:port`, obtain a tunnel (directly to origin
  for `direct`, or via `CONNECT`-to-upstream negotiating auth first for `upstream`/`pac`), reply
  `200 Connection Established`, then splice bytes bidirectionally until either side closes.
- **Per-connection isolation:** each accepted connection is its own tokio task with its own
  buffers/state. A failing/hung connection must not affect others or leak.
- **Graceful shutdown:** a shutdown signal stops the accept loop promptly and lets in-flight
  connections drain within a bounded timeout, then aborts stragglers. No hang, no panic. (This
  is the direct lesson of the prior build's listener-shutdown deadlock — see §8, LESSON-2.)

### 5.2 Upstream auth (`zicade-auth` + `zicade-win`)

- **Auth modes:** `none`, `basic`, `negotiate`.
- **Negotiate handshake state machine** (in `zicade-auth`, pure/testable): drives the
  challenge/response legs against the upstream 407. It calls an injected
  `UpstreamAuthenticator` trait (real impl in `zicade-win` via SSPI; fake impl in tests).
- Must correctly handle a **multi-leg 407 handshake** (NTLM is 3-leg: negotiate → challenge →
  authenticate). Bound the number of legs (reject after N to avoid infinite loops).
- **Credential-handle lifetime:** acquire and dispose SSPI handles correctly; a handle must
  outlive the whole handshake for a connection and be released after. No reuse-after-free, no
  leak (Rust's ownership handles the safety; tests assert the *lifecycle* — acquired once,
  released once).
- Auth applies both to `CONNECT` (tunnel setup) and to plain HTTP forwarding through an
  upstream.

### 5.3 Configuration + web UI (`zicade-config` + `zicade-web`)

Config is **web-UI-first** (redesigned; not byte-compatible with the old v1 JSON, though it
carries the same *concepts*). Model (serde):

```jsonc
{
  "listen":  { "host": "127.0.0.1", "port": 3129 },
  "routing": {
    "mode": "pac",                                  // "direct" | "upstream" | "pac"
    "upstream": { "host": "wp8080", "port": 8080,
                  "auth": { "mode": "negotiate" } },
    "pac":      { "source": "auto",                 // "auto" | "file" | "url"
                  "path": null, "url": null,
                  "failPolicy": "error",            // "error" | "direct"
                  "auth": { "mode": "negotiate" } } // applied to PAC-selected upstreams
  },
  "logging": { "level": "info", "format": "json" }
}
```

- `auth.mode` `negotiate` is **Windows-only**; validation rejects it elsewhere.
- **PAC-selected upstreams must inherit `routing.pac.auth`** — a PAC result that points at the
  gateway has to be able to authenticate (see §8, LESSON-6).
- **Web UI (`zicade-web`, axum + htmx):**
  - View + edit the full config through forms; changes validated server-side and persisted;
    apply either via hot-reload of a config snapshot or a clean restart of the proxy listener.
  - **Live log streaming** via SSE from `zicade-observe` (a `tracing` layer pushes structured
    events into a bounded ring buffer + a broadcast channel; the SSE endpoint tails it).
  - Status panel: current routing mode, listen address, upstream reachability, active
    connection count, request counters, last error.
  - **Security posture: loopback-only + local token.** Bind API + UI to `127.0.0.1` only. All
    **mutating** requests require a token; the token is generated at first run and written to a
    user-readable local file (e.g. `%LOCALAPPDATA%\Zicade\ui-token`). Read-only status/log
    endpoints may be open on loopback. Include a CSRF-safe pattern for htmx form posts
    (token header). Never bind to a non-loopback interface in this milestone.

### 5.4 Binary/lifecycle (`apps/zicade`)

- Loads config, starts the proxy and the web server, wires shutdown (Ctrl-C / service stop) to
  graceful shutdown of both.
- Structured startup diagnostics: if the listen port is taken or config invalid, fail fast with
  a clear message (and surface it in the UI if the web server is up).
- (Stretch, later milestone) Windows tray + service hosting, mirroring the prior build. Not in
  the MVP critical path.

---

## 6. Coding rules / constraints

- **`unsafe` only in `zicade-win`.** Deny `unsafe` elsewhere: `#![forbid(unsafe_code)]` in
  every crate except `zicade-win`. `zicade-win` isolates and documents each unsafe block with a
  `// SAFETY:` comment.
- **No logic duplication for SSPI/WinHTTP** — it lives only in `zicade-win`.
- **Delete dead code when you replace it.** When behavior moves, remove the old path in the same
  commit. No commented-out corpses.
- **Windows string boundaries:** be deliberate about UTF-8 (Rust) vs UTF-16 (`PCWSTR`) vs
  ANSI/`PCSTR` at every Win32 call; wide-encode at the boundary. (This replaces the prior
  build's `[:0]u8` null-terminator hazards.)
- **`clippy` clean** (`cargo clippy --all-targets -- -D warnings`) and **`rustfmt`** enforced.
- **Errors are typed** (`thiserror` per crate; `anyhow` only in the binary). No `unwrap()` in
  library code paths that can fail at runtime; `unwrap`/`expect` allowed in tests and on
  genuinely-infallible invariants (documented).
- **No blocking calls on the async runtime** — wrap blocking Win32/WinHTTP calls in
  `spawn_blocking` where they can block.

---

## 7. Test strategy

Three tiers, all under `cargo test`:

1. **Unit tests** (in-crate `#[cfg(test)]`): pure logic — config validation, routing decisions,
   the Negotiate state machine driven by a fake authenticator, PAC-result parsing, request-line
   rewriting, token/CSRF checks. These run on any OS with no network.
2. **Integration tests** (`tests/` per crate + a workspace-level `tests/`): spin up the real
   proxy on an ephemeral loopback port and drive it with a real HTTP client against
   **in-process fake upstream/origin servers** (tokio TCP fixtures / `wiremock`). Cover:
   - direct mode HTTP + HTTPS(CONNECT) round-trips,
   - upstream mode with a fake 407→challenge→200 upstream (fake authenticator injected),
   - PAC mode with a fake PAC backend returning DIRECT for "internal" and an upstream for
     "external",
   - **POST body integrity** (small and large/chunked bodies echoed back unchanged),
   - **concurrency**: N simultaneous connections all succeed,
   - **no leak under load**: after M sequential + concurrent requests, active task/connection
     count and fd/handle usage return to baseline,
   - **graceful shutdown**: shutdown while connections are in flight completes within the
     timeout, no panic, no hang.
3. **Live-environment tests** — gated behind **`ZICADE_LIVE_PROXY=1`** (skipped by default,
   skip logged explicitly). Run only on the domain-joined Windows box. Cover the real SSPI
   Negotiate handshake against `wp8080`, real PAC resolution (internal→DIRECT, external→
   upstream), real HTTPS via CONNECT through the gateway, and a soak (sequential + concurrent,
   stable handles/memory). Provide a way to point these at the real `wp8080` and PAC via env.

**Windows SSPI in tests without the domain:** provide a loopback SSPI harness in `zicade-win`
tests — a real `AcceptSecurityContext` (inbound cred) paired with `InitializeSecurityContext`
(outbound cred) on the same machine to produce genuine tokens, so the handshake code is
exercised for real without a remote server. (This is the lesson of the prior build's
"fake token" tests failing with `SEC_E_INVALID_TOKEN` — see §8, LESSON-3.)

---

## 8. Hard-won lessons — encode each as a regression test **first**

These are real bugs found against the live environment in the Zig build. Most are *memory* bugs
that Rust's ownership model prevents by construction — but the **behavior each bug corrupted must
still be pinned by a test**, because the domain hazard (protocol legs, body framing, shutdown,
PAC auth, leaks) is language-independent. Write these tests in the red backlog up front.

| # | Prior bug (Zig) | Root cause | Rust status | Test to write FIRST |
|---|---|---|---|---|
| LESSON-1 | Process died after 1–2 connections | use-after-free tearing down a connection (defer reverse-order freed host/tracker still in use) | prevented by ownership | Serve many sequential connections; assert process survives and each response is correct; assert connection bookkeeping is cleaned up. |
| LESSON-2 | Shutdown hung / panicked | `shutdown()` on a *listening* socket → `INVALID_PARAMETER`; closing socket mid-`accept` → cancelled-accept panic | design choice, not free | Graceful-shutdown test: signal shutdown while accepting + while a connection is in flight; must complete within timeout, no hang, no panic. |
| LESSON-3 | SSPI unit tests failed `SEC_E_INVALID_TOKEN` | tests fed hand-made fake tokens SSPI rejects | test-infra design | The loopback SSPI harness (§7) producing genuine tokens; assert a full handshake completes. |
| LESSON-4 | POST bodies corrupted | self-referential buffer pointers dangled after a by-value return | prevented by ownership | Echo POST test: small + large + chunked bodies round-trip **byte-identical** through the proxy. |
| LESSON-5 | `AddressInUse` handling / AFD bind quirks | Windows listener socket edge cases | platform behavior | Startup-failure test: binding an already-taken port fails fast with a clear typed error (not a panic). |
| LESSON-6 | PAC mode crashed + couldn't authenticate | PAC-selected route host dangled (freed arena); PAC routes had no auth config | prevented by ownership + config fix | PAC test: external host resolves to the upstream **and** carries `routing.pac.auth`, so the handshake runs; internal host → DIRECT with no auth. Assert no crash and auth applied. |
| LESSON-7 | Handle leak (grew ~171→231 over 60 reqs) | completed connection threads never reaped | design choice | Leak test: run 60+ mixed requests; assert active task count + OS handle/fd count return to baseline (±small tolerance). |

---

## 9. Milestones (each = write red backlog, then drive green)

**M0 — Workspace & harness.** Cargo workspace, crates scaffolded, CI-style
`cargo fmt --check && cargo clippy -D warnings && cargo test` runs green (with a trivial test
per crate). `#![forbid(unsafe_code)]` everywhere except `zicade-win`.

**M1 — Config + validation.** `zicade-config` schema, load/save, full validation (incl.
negotiate-is-Windows-only, PAC auth inheritance). All unit tests green. *No proxy yet.*

**M2 — Direct-mode proxy.** `zicade-proxy` accept loop + HTTP forward + CONNECT tunnel in
`direct` mode against fake origin. Green: HTTP round-trip, HTTPS/CONNECT round-trip, POST
integrity (LESSON-4), concurrency, leak-under-load (LESSON-7), graceful shutdown (LESSON-2),
startup-failure (LESSON-5).

**M3 — Upstream mode + Negotiate.** `zicade-auth` handshake state machine (fake authenticator)
green first; then `zicade-win` SSPI impl with the loopback harness (LESSON-3); multi-leg 407,
handle lifecycle, CONNECT-through-upstream, HTTP-forward-through-upstream all green.

**M4 — PAC mode.** `zicade-routing` PAC resolver trait + fake backend green; `zicade-win`
WinHTTP backend; internal→DIRECT / external→upstream-with-auth (LESSON-6) green.

**M5 — Web UI + API.** `zicade-observe` tracing→ring-buffer→broadcast; `zicade-web` axum API
(config CRUD with validation, status, SSE logs), htmx frontend, loopback+token auth + CSRF.
Green: token required for mutations, config edit persists + applies, SSE streams logs, status
reflects live state.

**M6 — Binary integration + live baseline.** `apps/zicade` wires everything; lifecycle +
graceful shutdown end-to-end. Then, on the domain-joined box, turn the **`ZICADE_LIVE_PROXY=1`**
suite green against real `wp8080` + real PAC: HTTPS via CONNECT through the gateway, real
Negotiate, internal→DIRECT/external→upstream, and a soak with stable handles/memory.

**M7 (stretch) — Tray/service hosting**, config hot-reload polish, metrics.

---

## 10. Definition of done

- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test` all green
  on Windows and on a non-Windows CI host (live + Windows-SSPI tests skipped with explicit,
  logged skips off-target).
- Every LESSON in §8 has a dedicated green regression test.
- The `ZICADE_LIVE_PROXY=1` suite passes on the domain-joined Windows machine against real
  `wp8080` + PAC.
- Web UI: config editable + persisted, mutations token-gated, logs stream live, status accurate.
- No `unsafe` outside `zicade-win`; no SSPI/WinHTTP logic outside `zicade-win`; no file grossly
  oversized; no dead code.
- A short `README` documenting how to run, how to point tests at the live proxy, and how to
  reach the web UI (URL + where the token file lives).

---

## 11. Deliverables (final report)

When done, summarize: milestones completed with test counts (unit/integration/live), each
LESSON with its regression test, the live baseline results (sequential/concurrent/HTTPS/POST,
handle+memory stability), the config schema, the web UI security model, and any residual
skips/known-limitations with justification.

---

### Appendix A — environment quick facts
- Target: domain-joined **Windows 11**, corporate upstream proxy at **`wp8080`** (407 until
  Negotiate). PAC decides DIRECT vs upstream per host. **Zicade → `wp8080` directly; never via a
  jump host.** SSPI can get an outbound cred handle with no stored password. HTTPS = CONNECT
  tunnel; HTTP = forward.
- Live tests gated behind `ZICADE_LIVE_PROXY=1`; SSPI-real tests skipped off-Windows.

### Appendix B — why Rust (the rebuild rationale)
4 of the 6 bugs in the prior Zig build were memory-safety bugs (use-after-free / dangling
self-referential buffers / dangling PAC host). Rust's borrow checker rejects that entire class
at compile time. The mature `windows-rs` / `tokio` / `hyper` / `axum` ecosystem replaces
hand-rolled SSPI FFI and a churning async std API. The protocol/domain bugs (multi-leg 407,
PAC-auth inheritance, framing, leaks, shutdown) are *not* language bugs — hence §8 pins them
with tests regardless of language.
