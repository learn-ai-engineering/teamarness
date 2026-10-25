# Updater signature fixtures

These are public, nonsecret qualification bytes, not application updates or production trust material.

CLI `@tauri-apps/cli` 2.12.4 generated a disposable key in a private temporary directory on 2026-10-24. The key signs the literal `artifact.bin` bytes. `versioned.sig` uses the manual `signer sign --app-version 1.2.3` path. `unversioned.sig` uses the same signer without `--app-version`. `changed-comment.sig` changes `version:1.2.3` to `version:9.9.9` inside the decoded trusted comment without re-signing it. The public key is `public-key.txt`. The private key is never part of this directory or repository.

`examples/qualify-update.rs` tests these through the official plugin, with independently expected tag `v1.2.3` and both `darwin-aarch64` and `darwin-aarch64-app` selections. The inflated-version case expects tag `v9.9.9` but supplies the genuine `1.2.3` signature. This must fail in the official verifier, not just at the tag check.

Run `cargo test -p teamarness-desktop --locked` after `npm ci && npm run build`. The example has `test = true`, so the normal desktop test command runs its cases. `cargo run --locked -p teamarness-desktop --example qualify-update -- --help` describes the bounded offline file interface. It opens no window, makes no request, and never calls an installer. Normal bundle-signer qualification uses the actual generated archive separately; these small fixtures do not prove bundle integrity or installed update behavior.
