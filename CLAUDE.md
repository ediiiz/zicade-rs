# Zicade

Local HTTP/HTTPS forward proxy for a domain-joined Windows box: authenticates to
the corporate upstream (SSPI Negotiate/NTLM or Basic), routes per request via
PAC, and has a native gpui-kit window (`zicade-ui`) that talks to the running
app in-process through `zicade-control`. `zicade-rs-PROMPT.md` is the original
build spec and the source of the §8 lessons table; parts of it are stale (no
htmx, milestones are done), so trust the code over it.

## Done means green

Run before calling any change done:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --workspace --lib --bins -- -D warnings -D clippy::unwrap_used -D clippy::expect_used
cargo test --workspace
bash scripts/check_file_size.sh
```

Locally this builds with the w64devkit GNU toolchain pinned in
`.cargo/config.toml`; CI uses MSVC (see `ci.yml`). A link error about `ld` or
`dlltool` is a config problem: fix it in `.cargo/config.toml`, never by editing
the global PATH.

## Working agreement

- **Red first.** Every behaviour change and bug fix starts with a test that
  fails for the right reason, then the smallest change that turns it green.
  Live-proxy tests stay behind `ZICADE_LIVE_PROXY=1`.
- **Split, don't silence.** When a function trips a complexity lint or a file
  passes 400 lines, extract a named helper or module. An `#[allow]` is the
  last resort and carries a `reason = "..."` a reviewer would accept.
- **`unsafe` lives only in `zicade-win`**, one `// SAFETY:` comment per block
  stating the invariant the call relies on. Every other crate is
  `#![forbid(unsafe_code)]`.
- **Errors are typed** (`thiserror` per crate, `anyhow` only in `apps/zicade`)
  and propagated or logged. A discarded `Result` is either a best-effort
  cleanup or it gets a `tracing` call.
- **Comments say why**, in terms of the behaviour the reader sees. Name the
  hazard ("a bogus Content-Length must not size an allocation"), not the
  milestone, lesson number, or issue that taught it.

## Invariants

These have each been broken once; tests pin them, keep them pinned.

- **Fail closed on routing and auth.** A poisoned lock, PAC error, or missing
  config never silently degrades to `Direct` or to no-auth. Recover poisoned
  mutexes with `PoisonError::into_inner`.
- **Peers don't size our buffers.** Bytes from a client, origin, or upstream
  are streamed or read under a fixed cap; network waits that can stall have a
  timeout.
- **The UI never sees secrets.** `Control::config_redacted` strips passwords;
  `Control::apply` keeps a saved password the UI left blank. Nothing but the
  proxy listens on a socket.
