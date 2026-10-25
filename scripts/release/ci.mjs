import { appendFile, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { assertEnvironment } from "./cli.mjs";
import { native } from "./artifacts.mjs";
import { assetNames } from "./manifest.mjs";
import { checkVersion } from "./version.mjs";

assertEnvironment();
const tag = `v${await checkVersion()}`;
console.log(JSON.stringify({ image: process.env.ImageOS, imageVersion: process.env.ImageVersion, architecture: process.arch, node: process.version, npm: native("npm", ["--version"]), rust: native("rustc", ["-Vv"]), cargo: native("cargo", ["--version"]), xcode: native("xcodebuild", ["-version"]), sdk: native("xcrun", ["--show-sdk-version"]), os: native("sw_vers", []) }, null, 2));
if (process.env.TEAMARNESS_UPDATER_PUBLIC_KEY) await writeFile(join(process.env.RUNNER_TEMP, "teamarness-public-key.txt"), process.env.TEAMARNESS_UPDATER_PUBLIC_KEY, { mode: 0o600 });
if (process.env.GITHUB_OUTPUT) await appendFile(process.env.GITHUB_OUTPUT, `tag=${tag}\nassets<<TEAMARNESS_ASSETS\n${[...assetNames(tag), "qualification.json"].map((name) => `target/release/distribution/${name}`).join("\n")}\nTEAMARNESS_ASSETS\n`);
