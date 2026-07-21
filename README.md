# Zicade

Zicade is a small, local forwarding **HTTP/HTTPS proxy** for Windows with a
loopback web UI. It listens on `127.0.0.1`, forwards plain HTTP (absolute-form →
origin-form) and tunnels `CONNECT`, and can route either directly to origins or
through an upstream corporate proxy — including an SSPI **Negotiate** (Kerberos/
NTLM) handshake against that upstream. A companion local web server exposes a
JSON API and a browser UI for editing the config, watching live logs, and
reading status.

The workspace is a strict-TDD Cargo workspace:

| Crate | Responsibility |
| --- | --- |
| `zicade-config` | Config schema, load/save, validation |
| `zicade-routing` | PAC parsing + `RouteResolver` seam |
| `zicade-auth` | Upstream authenticator + Negotiate handshake state machine |
| `zicade-win` | The **only** crate with `unsafe`/FFI: SSPI + WinHTTP PAC |
| `zicade-proxy` | The proxy data path (accept loop, forwarding, tunneling, shutdown) |
| `zicade-observe` | `tracing` layer → in-memory ring buffer + broadcast for the UI |
| `zicade-web` | axum JSON API + UI, token-gated mutations |
| `apps/zicade` | The binary: wires everything together and owns the lifecycle |

## Build prerequisites

- **Target toolchain:** `x86_64-pc-windows-gnu`
  (`rustup target add x86_64-pc-windows-gnu`). The build is pinned to this target
  via `.cargo/config.toml`.
- **w64devkit on PATH:** `C:\Users\<you>\tools\w64devkit\bin` must be reachable.
  The GNU toolchain there provides the linker (`gcc` → `collect2` → `ld`) and
  `dlltool`. `.cargo/config.toml` wires the linker, `COMPILER_PATH`, and
  `-Cdlltool=...` for the target build, **but host proc-macro dependencies**
  (e.g. `rust-embed`) still need `dlltool`/`ld` on `PATH`, so prepend that bin
  directory before running cargo:

  ```sh
  export PATH="/c/Users/<you>/tools/w64devkit/bin:$PATH"
  ```

- **crates.io reachability:** the initial dependency download must reach
  crates.io. On the corporate network this is the catch: **dependency downloads
  need an off-corp connection** (or a working proxy), while the **live proxy
  tests** below must run **on-corp** against the real upstream. Build/vendor deps
  off-corp first, then run the live suite on-corp.

## Running

```sh
cargo run -p zicade
```

On startup the binary:

1. Resolves the config path (first CLI argument, else the default below).
2. Loads the config, or writes the defaults on first run.
3. Installs the tracing subscriber (level/format from `logging`).
4. Binds the proxy and the web server on loopback, then serves until `Ctrl-C`
   (graceful shutdown drains in-flight connections).

Pass an explicit config path if you like:

```sh
cargo run -p zicade -- C:\path\to\config.json
```

### Addresses

- **Proxy:** `config.listen` — default `127.0.0.1:3129`.
- **Web UI / API:** the **proxy port + 1**, same host — default `127.0.0.1:3130`.

Point your browser or system proxy at the proxy address; open the web address in
a browser for the UI.

### Default config location

`%LOCALAPPDATA%\Zicade\config.json` (created with defaults on first run).

### Config schema (brief)

```jsonc
{
  "listen":  { "host": "127.0.0.1", "port": 3129 },
  "routing": {
    "mode": "direct",                 // "direct" | "upstream" | "pac"
    "upstream": {                      // required when mode = "upstream"
      "host": "wp8080", "port": 8080,
      "auth": {
        "mode": "negotiate",           // "none" | "basic" | "negotiate"
        "username": null, "password": null
      }
    },
    "pac": {                           // required when mode = "pac"
      "source": "auto",                // "auto" | "file" | "url"
      "path": null, "url": null,
      "failPolicy": "error",           // "error" | "direct"
      "auth": { "mode": "negotiate" }  // inherited by PAC-selected upstreams
    }
  },
  "logging": { "level": "info", "format": "json" }
}
```

Validation runs at startup and **fails fast** with a clear message: ports must
be non-zero, `upstream`/`pac` sections are required for their modes, and
`negotiate` auth is only accepted on Windows.

## Web UI

- **URL:** `http://127.0.0.1:3130/` (proxy port + 1).
- **Read-only** endpoints (`GET /api/config`, `GET /api/status`,
  `GET /events/logs` SSE) are open on loopback.
- **Mutations** (`PUT /api/config`) require the local UI token in the
  `X-Zicade-Token` header. That custom-header requirement doubles as CSRF
  protection.
- **Token file:** `%LOCALAPPDATA%\Zicade\ui-token`, created on first run
  (32 hex chars). Read it from that file and send it as `X-Zicade-Token`.

## Testing

```sh
export PATH="/c/Users/<you>/tools/w64devkit/bin:$PATH"
cargo test            # full workspace suite (deterministic, no network)
```

The default suite is hermetic: no real upstream network, ephemeral loopback
ports, fake origins/backends.

### Live proxy suite (gated, on-corp)

The live tests are gated behind an env var and are intended for the
domain-joined Windows machine, on the corporate network, pointing at the real
`wp8080` upstream + PAC:

```sh
ZICADE_LIVE_PROXY=1 cargo test
```

Run these only on-corp; off-target they are skipped. Verified green against the
real Skyhigh/McAfee Secure Web Gateway at `wp8080:8080`: the multi-leg
`407 -> NTLM -> 200` handshake, HTTPS via `CONNECT` through the gateway,
plain-HTTP forwarding, and a soak that holds a flat OS-handle count (no leak).

The live PAC check is tolerant of hosts with no WPAD/PAC autoconfig (a common
corporate setup that pushes routing via GPO or a static proxy): it logs a skip
rather than failing when `WinHttpGetProxyForUrl` reports no discoverable script.

## Known limitations

- **PAC data-path routing is wired.** In `mode = "pac"` the proxy resolves each
  request through WinHTTP and connects DIRECT or through the resolved upstream
  (applying `routing.pac.auth`, LESSON-6). `source = "auto"` uses WPAD
  auto-detect; hosts with no discoverable WPAD script need `source = "url"` (or
  `"file"`) to point at the PAC explicitly. `failPolicy` governs resolution
  failures: `"direct"` falls back to a DIRECT connection, `"error"` fails the
  request with `502`. PAC results are not cached (each request re-resolves).
- **Upstream connections are authenticated per request, not pooled.** Because
  Negotiate/NTLM authenticates the TCP connection (not the request), each
  HTTP-forward request opens a fresh upstream connection, completes the multi-leg
  407 handshake, and tears it down. This is an intentional, correct, leak-free
  design (LESSON-7): the per-request connect/auth/teardown holds a flat OS-handle
  steady state, locked in by regression tests. Reusing an authenticated upstream
  connection across requests on the same client keep-alive connection would save
  the handshake cost, but is **deliberately deferred** — it adds per-connection
  stream state, mid-reuse reconnect/re-auth, and keep-alive framing complexity
  with real risk to the handle-stability guarantee, for a latency win that does
  not justify it. `CONNECT` tunnels are inherently per-connection and unaffected.
- **Basic upstream auth is not wired.** `auth.mode = "basic"` currently logs a
  warning and proceeds **without** proxy authentication. Use `negotiate` (or
  `none`). `none` and `negotiate` are fully wired.
- **`negotiate` is Windows-only.** Off-Windows, config validation rejects it; if
  a Negotiate authenticator is somehow constructed off-target, it errors at
  handshake time rather than at startup.
- **`negotiate` uses the NTLM SSPI package, not SPNEGO/Kerberos.** The target
  corporate gateway (Skyhigh/McAfee) offers `Negotiate`/`NTLM`/`Basic` but does
  not accept SPNEGO, and has no Kerberos SPN for the proxy appliance. The SSPI
  `Negotiate` package's raw-NTLM fallback also cannot complete against it
  (`SEC_E_INVALID_TOKEN` on the challenge leg). We therefore acquire the SSPI
  credential with the **NTLM** package, which completes the standard three-leg
  NTLM handshake; the gateway accepts the token under the `Negotiate` scheme.
  Environments that require Kerberos SSO would need this package made
  configurable.
