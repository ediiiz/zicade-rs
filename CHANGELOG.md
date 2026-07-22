# Changelog

## [0.2.1](https://github.com/ediiiz/zicade-rs/compare/v0.2.0...v0.2.1) (2026-07-22)


### Bug Fixes

* **installer:** resolve ICE57/ICE69 in the WiX source ([c64913f](https://github.com/ediiiz/zicade-rs/commit/c64913fb1aefc7264f2a8e1525e7eed77cfff045))
* **installer:** resolve ICE57/ICE69 in the WiX source ([85c5c83](https://github.com/ediiiz/zicade-rs/commit/85c5c8359fd3f145126190da0cff3bbc4bbef24e))

## [0.2.0](https://github.com/ediiiz/zicade-rs/compare/v0.1.0...v0.2.0) (2026-07-22)


### Features

* add Command::Default + pure launch_mode dispatch ([b4b2735](https://github.com/ediiiz/zicade-rs/commit/b4b273565be0d9c0d2868c877611ffdda5cb2a8e))
* add Zicade logo and use it as the app icon ([e57f4fb](https://github.com/ediiiz/zicade-rs/commit/e57f4fb45bc859e003e18f60b9fec9cb2daf8a43))
* **app:** add pure on_corp DNS-suffix matcher (netmon) ([5a59111](https://github.com/ediiiz/zicade-rs/commit/5a59111e353d1dfc1d566b668d0e852ee70fb96e))
* apply Web UI routing edits to the live proxy without a restart ([bd74c52](https://github.com/ediiiz/zicade-rs/commit/bd74c5213294c63d44d1248884e4674be85970c2))
* **app:** map AuthMode::Basic onto UpstreamAuth::Basic ([ec399d3](https://github.com/ediiiz/zicade-rs/commit/ec399d359a6f1b16cfd6afa8ec633f147a3a9e64))
* **app:** NetworkGate supervisor gates routing on corp-network detection ([fb59046](https://github.com/ediiiz/zicade-rs/commit/fb590462a8e0a98c73af3992e82f1e133605585c))
* **auth:** add build_basic_authorization helper ([80fd2e1](https://github.com/ediiiz/zicade-rs/commit/80fd2e16f527b1e6b0e640734ec05d2513836d54))
* build a WinHTTP-backed PacRouter for mode = pac ([f97d58e](https://github.com/ediiiz/zicade-rs/commit/f97d58ed7db7cf0c4d965fe92418aff0f03facb0))
* **config:** add AuthConfig.package (SspiPackage, default ntlm) ([de0ab1d](https://github.com/ediiiz/zicade-rs/commit/de0ab1d7da994535a3cbba19ff0b3cd6bcae5c94))
* **config:** add routing.corpNetwork gate schema + validation ([575e74b](https://github.com/ediiiz/zicade-rs/commit/575e74b2cb51321f61b1bbf4d9e88c28a4ecef42))
* **config:** add web.authRequired option, default off ([53158d8](https://github.com/ediiiz/zicade-rs/commit/53158d89bfcaa03c316b1524c96818225002ddb5))
* **config:** reject basic auth without a username ([848d447](https://github.com/ediiiz/zicade-rs/commit/848d447122b93e4cfe4ee0df74f88d3c48f34140))
* dispatch console/service/install/uninstall in main ([8f8dc47](https://github.com/ediiiz/zicade-rs/commit/8f8dc4751651cef50ec95c967222aaebe0f073c4))
* implement arg parser dispatch table ([e38b1ee](https://github.com/ediiiz/zicade-rs/commit/e38b1eef616628cb3c8cc47ae419f8ff2b24eaf1))
* implement pure PAC discovery strategy + static bypass match ([91672fe](https://github.com/ediiiz/zicade-rs/commit/91672fe635dc0a55cbd713e89414a8dbd26b4119))
* **installer:** add MSI package with scope choice + startup options ([8cdcf76](https://github.com/ediiiz/zicade-rs/commit/8cdcf763e88856202aa1c2580b5bf76ba1582627))
* **m0:** workspace scaffold, guardrails, and windows-rs GNU link proof ([9bdc95d](https://github.com/ediiiz/zicade-rs/commit/9bdc95d1eb013b90b459cbd908fe73be0248ced8))
* **m1:** zicade-config schema, validation, and PAC-auth inheritance ([0f63666](https://github.com/ediiiz/zicade-rs/commit/0f636665b683ed45616d962092caa14e6186bc12))
* **m2:** direct-mode proxy — HTTP forward + CONNECT tunnel ([ff41980](https://github.com/ediiiz/zicade-rs/commit/ff41980d1a2f16c10855bd02f825f14d368e3eaf))
* **m3a:** pure Negotiate handshake state machine + header codec ([52d9433](https://github.com/ediiiz/zicade-rs/commit/52d94335d0579ac2f01210fa3d12622444849401))
* **m3b:** real SSPI Negotiate authenticator + loopback harness (LESSON-3) ([f28756a](https://github.com/ediiiz/zicade-rs/commit/f28756adbec873024d620dd2a8d74ce530f5ff99))
* **m3c:** upstream-mode proxy — CONNECT + HTTP forward through Negotiate 407 upstream ([0c5fd4f](https://github.com/ediiiz/zicade-rs/commit/0c5fd4f06027f891d8b2c3283f81b8f568cf2488))
* **m4:** PAC routing — resolver trait, fake backend, WinHTTP backend, LESSON-6 ([ca0f021](https://github.com/ediiiz/zicade-rs/commit/ca0f021ec9ad2584f201898450182e0afbead0f9))
* **m5:** observability ring-buffer/SSE + axum config UI with token+CSRF ([e4e60ab](https://github.com/ediiiz/zicade-rs/commit/e4e60ab4b17d5307815592fe894d4edadddf4075))
* **m6:** binary wiring, graceful shutdown, README ([669aa27](https://github.com/ediiiz/zicade-rs/commit/669aa270b2253fe4ce03377c6617779642ba9678))
* network throughput chart via SSE metrics stream ([38ad6d2](https://github.com/ediiiz/zicade-rs/commit/38ad6d28d88204f59b009d20283dff5e7d72f874))
* **proxy:** wire HTTP Basic upstream authentication ([417e018](https://github.com/ediiiz/zicade-rs/commit/417e0183fa941e1b87ec89e18c20dc8cbe4178b7))
* race-free ServiceStop::wait ([d0c8b9d](https://github.com/ediiiz/zicade-rs/commit/d0c8b9daf4857e1568e9c01cf17df1c3a4c0b0d6))
* reflect live-applied routing mode in GET /api/status ([6a12dcc](https://github.com/ediiiz/zicade-rs/commit/6a12dcc5c49d54831820cc6a77ff6d93921ecb81))
* report live proxy metrics on GET /api/status ([0c5d9d2](https://github.com/ediiiz/zicade-rs/commit/0c5d9d2eea6de3f10c36e09efd90ccadf974597c))
* SCM service module in zicade-win (install/uninstall/dispatcher) ([66486db](https://github.com/ediiiz/zicade-rs/commit/66486db56f1afb5cbf43406ad4fd37617aab667e))
* source=auto discovers per-user AutoConfigURL (IE proxy config) ([5fd9efa](https://github.com/ediiiz/zicade-rs/commit/5fd9efabc1794b5c4790dcdb339bfd4e14ce81d3))
* tray menu items + id-to-action mapping ([053c507](https://github.com/ediiiz/zicade-rs/commit/053c507abcd2abd822924ef4fcb868d85465ce5e))
* tray-mode dispatch on double-click launch ([a81477f](https://github.com/ediiiz/zicade-rs/commit/a81477fae36d2f2124d85dde1b9f288119046360))
* **web:** gate PUT on web.authRequired, off by default ([3464c6d](https://github.com/ediiiz/zicade-rs/commit/3464c6d52834b7fe45f5a2a93ef81f3cd90ea7ca))
* **web:** live status panel with polling ([c7afe2e](https://github.com/ediiiz/zicade-rs/commit/c7afe2edee1f80fb07c827dfa387141403bd9158))
* **web:** rebuild UI with Pico, full config form + progressive disclosure ([5fbdf1e](https://github.com/ediiiz/zicade-rs/commit/5fbdf1e28a2640ff8cabce520e49f064c8caf228))
* **web:** surface on-corp state + corpNetwork config form; fix port-race flake ([3362310](https://github.com/ediiiz/zicade-rs/commit/3362310b2c01119c25c1204c4047f0e5bcd1ee81))
* **web:** vendor Pico CSS v2.1.1 as an embedded, same-origin asset ([c13615e](https://github.com/ediiiz/zicade-rs/commit/c13615ebcf6a85f1b43f0a2725b12112f607eddb))
* **win:** add netmon FFI — adapter DNS suffixes + address-change wait ([e6252c4](https://github.com/ediiiz/zicade-rs/commit/e6252c4768f1c1273b376a68adc2877ceb52c05d))
* wire per-request PAC routing into the proxy data path ([19be3bb](https://github.com/ediiiz/zicade-rs/commit/19be3bb1bce7e2c9603565efa0fbab54c231b2e0))
* zicade-win system-tray FFI (icon, menu, console, browser) ([0226716](https://github.com/ediiiz/zicade-rs/commit/02267165eacd5471df850dde85f0b910dd432ea0))
* **zicade-win:** select SSPI package at runtime (NTLM default) ([2d6dd46](https://github.com/ediiiz/zicade-rs/commit/2d6dd46e1adf599c95bf95d9c375bc33a19e9927))
* **zicade:** wire config SSPI package into the SSPI factory ([b187d95](https://github.com/ediiiz/zicade-rs/commit/b187d95dae82c72bc05b46bdcb31b3fafd70a2d0))


### Bug Fixes

* **auth:** use NTLM SSPI package for upstream proxy auth (live vs wp8080) ([dd1c3d9](https://github.com/ediiiz/zicade-rs/commit/dd1c3d975fbeadee5d808d8ff6642774d8b48826))
* end SSE streams on shutdown so graceful shutdown completes ([413767a](https://github.com/ediiiz/zicade-rs/commit/413767a14ecf4b0fd7e6f482b56c83807d2adef0))
* end SSE streams on shutdown so graceful shutdown completes ([8df7346](https://github.com/ediiiz/zicade-rs/commit/8df73464822b622427414e8c6812777b38901f7b))
* **m0:** make GNU toolchain self-contained + shared target dir ([2c60ae7](https://github.com/ediiiz/zicade-rs/commit/2c60ae7fe2504b8e65c751d1513c29f87d644f3a))


### Documentation

* README reflects PAC per-request routing is now wired ([fec0045](https://github.com/ediiiz/zicade-rs/commit/fec00455a3dce49d514d4c67ab407a58c6badb79))
* **readme:** document routing.corpNetwork gate ([5359969](https://github.com/ediiiz/zicade-rs/commit/53599692745dcdff36426cf08b47f8fd88b4130f))
* **readme:** document system-tray mode (double-click launch) ([2029692](https://github.com/ediiiz/zicade-rs/commit/202969253ea5dd1b68593448b7abc85bb2af77cf))
* **readme:** document web.authRequired (default off) + expanded UI ([61128d4](https://github.com/ediiiz/zicade-rs/commit/61128d4992237de9420ebb3a1641c6b001595e6f))
* **readme:** record live results + NTLM package rationale ([92ac1fb](https://github.com/ediiiz/zicade-rs/commit/92ac1fb56699eb2599dabe43d9b306f8c0169e18))
* **readme:** reflect Basic auth wired + configurable SSPI package ([00941c3](https://github.com/ediiiz/zicade-rs/commit/00941c3ba4e5620a818599c491f97ba29201d7c1))
* record per-request upstream auth as intentional; defer pooling ([308e38f](https://github.com/ediiiz/zicade-rs/commit/308e38f3262acd1451c5f5b7845a3ea235e1aab7))
