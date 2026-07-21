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

Run these only on-corp; off-target they are skipped.

## Known limitations

- **PAC data-path routing is not yet wired.** In `mode = "pac"` the app builds
  and validates the WinHTTP `RouteResolver`, logs that PAC is configured, and
  then runs the proxy in **Direct** mode. Per-request PAC selection in the proxy
  data path is deferred (the resolver + WinHTTP backend themselves are covered by
  tests). Use `mode = "upstream"` for a fixed upstream proxy.
- **Basic upstream auth is not wired.** `auth.mode = "basic"` currently logs a
  warning and proceeds **without** proxy authentication. Use `negotiate` (or
  `none`). `none` and `negotiate` are fully wired.
- **`negotiate` is Windows-only.** Off-Windows, config validation rejects it; if
  a Negotiate authenticator is somehow constructed off-target, it errors at
  handshake time rather than at startup.
```
