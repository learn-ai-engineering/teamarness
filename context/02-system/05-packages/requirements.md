# 02-system/05-packages — Package Requirements

Prefix: `RES.SYS.PKG`

*Requirements for this subsystem will be authored during Phase 2 scoping. The package system defines manifests, semantic capabilities, the package lifecycle, the TypeScript SDK, and the trust model. All user-visible capability lives in packages. Package UIs are TypeScript web content rendered in Tauri webviews. Package backends are Rust modules with crate-level isolation from core internals. Communication crosses the Tauri invoke/event bridge through typed, secret-free capability contracts.*