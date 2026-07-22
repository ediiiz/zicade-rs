//! Embed the Windows app icon into the `zicade` executable.
//!
//! [`zicade.rc`](zicade.rc) names `assets/zicade.ico` as resource id 1.
//! `embed-resource` compiles it with the active toolchain's resource compiler —
//! `windres` on the pinned GNU target, `rc.exe` on MSVC (CI) — and links it in,
//! so Explorer, the taskbar/console window, and the system tray all show the
//! Zicade logo. It no-ops on non-Windows targets. Icon embedding is cosmetic, so
//! a missing resource compiler downgrades to a warning rather than failing the
//! build.

fn main() {
    println!("cargo:rerun-if-changed=zicade.rc");
    println!("cargo:rerun-if-changed=assets/zicade.ico");

    if let Err(err) = embed_resource::compile("zicade.rc", embed_resource::NONE).manifest_optional()
    {
        println!("cargo:warning=zicade: app icon not embedded: {err}");
    }
}
