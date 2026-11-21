// Keep source-built test fixtures on the repository compiler, including registry-cwd children.
import { readFileSync } from "node:fs";

export function cargoFixtureEnvironment() {
  const channel = /^channel = "([^"]+)"$/m.exec(readFileSync(new URL("../rust-toolchain.toml", import.meta.url), "utf8"))?.[1];
  if (!channel) throw new Error("repository must pin a Rust toolchain channel");
  const env = { ...process.env, RUSTUP_TOOLCHAIN: channel };
  delete env.RUSTC;
  // ring tracks these injected Cargo-test variables as build inputs. Passing
  // caller metadata into nested Cargo recompiles/replaces live fixture binaries.
  for (const key of Object.keys(env)) if (key === "CARGO_MANIFEST_DIR" || key.startsWith("CARGO_PKG_")) delete env[key];
  return env;
}
