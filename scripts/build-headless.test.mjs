import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmod, copyFile, mkdir, mkdtemp, readFile, rm, stat, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { repositoryRoot, readBundledPackages } from "./bundled-packages.mjs";
import { verifyHeadlessDistribution } from "./headless-distribution.mjs";
import { removeSmokeDirectory } from "./smoke-headless.mjs";

const linux = "x86_64-unknown-linux-gnu", mac = "aarch64-apple-darwin";

test("artifact smoke removes its read-only admitted UI cache without following external symlinks", async () => {
  const parent = await mkdtemp(join(tmpdir(), "teamarness-smoke-cleanup-"));
  const disposable = join(parent, "disposable"), outside = join(parent, "outside");
  await mkdir(join(disposable, "data/packages/ui-cache/v1/digest/ui"), { recursive: true });
  await mkdir(outside);
  await writeFile(join(outside, "preserved"), "untouched", { mode: 0o400 });
  await chmod(outside, 0o500);
  await writeFile(join(disposable, "data/packages/ui-cache/v1/digest/ui/index.js"), "inert", { mode: 0o400 });
  await symlink(outside, join(disposable, "outside-link"));
  for (const path of ["data/packages/ui-cache/v1/digest/ui", "data/packages/ui-cache/v1/digest"]) await chmod(join(disposable, path), 0o500);
  try {
    await removeSmokeDirectory(disposable);
    await assert.rejects(readFile(join(disposable, "data/packages/ui-cache/v1/digest/ui/index.js")), /ENOENT/);
    assert.equal(await readFile(join(outside, "preserved"), "utf8"), "untouched");
    assert.equal((await stat(outside)).mode & 0o777, 0o500);
    assert.equal((await stat(join(outside, "preserved"))).mode & 0o777, 0o400);
  } finally {
    await chmod(outside, 0o700);
    await rm(parent, { recursive: true, force: true });
  }
});

// Real assembler, with only the expensive compiler/npm process boundary replaced.
// Verifies: TEAM.DEL-R07
test("headless assembly isolates cleanup, builds only selected resources, and verifies final native bytes", async () => {
  const root = await mkdtemp(join(tmpdir(), "teamarness-headless-build-"));
  try {
    const packages = await readBundledPackages();
    await mkdir(join(root, "packages"));
    await copyFile(join(repositoryRoot, "packages/bundled-packages.json"), join(root, "packages/bundled-packages.json"));
    const sourceManifests = new Map();
    for (const { sourceDirectory, manifest } of packages) {
      const source = join(root, sourceDirectory);
      await mkdir(join(source, "ui/dist"), { recursive: true });
      for (const file of ["teamarness.package.json", "ui-contract.json", ...(manifest.portContract ? [manifest.portContract.path] : [])]) {
        await copyFile(join(repositoryRoot, sourceDirectory, file), join(source, file));
      }
      sourceManifests.set(source, await readFile(join(source, "teamarness.package.json")));
      await writeFile(join(source, "ui/package.json"), JSON.stringify({ name: manifest.identity }));
      for (const { path } of [manifest.ui.entrypoint, ...manifest.ui.assets]) await writeFile(join(source, "ui/dist", path.slice(3)), "inert UI fixture");
    }
    const output = join(root, "target/teamarness-applications/headless", linux);
    const sentinels = [join(root, "target/teamarness-packages/desktop"), join(root, "target/debug/packages/debug"), join(root, "target/teamarness-applications/headless", mac, "mac"), join(output, "obsolete")];
    for (const path of sentinels) { await mkdir(join(path, ".."), { recursive: true }); await writeFile(path, "preserve"); }
    const tools = join(root, "tools"); await mkdir(tools);
    const tool = `#!${process.execPath}\nimport {appendFileSync, mkdirSync, writeFileSync} from 'node:fs';\nimport {join, basename} from 'node:path';\nconst root=${JSON.stringify(root)}, args=process.argv.slice(2), name=basename(process.argv[1]);\nappendFileSync(join(root,'commands'),JSON.stringify([name,args])+'\\n');\nif(name==='rustc') console.log('host: ${linux}');\nif(name==='cargo' && args[0]==='metadata') console.log(JSON.stringify({target_directory:join(root,'cache')}));\nif(name==='cargo' && args[0]==='build') { const dir=join(root,'cache','${linux}','release'); mkdirSync(dir,{recursive:true}); const bytes=Buffer.alloc(64); bytes.set([0x7f,0x45,0x4c,0x46,2,1]); bytes.writeUInt16LE(62,18); writeFileSync(join(dir,args[args.indexOf('-p')+1]),bytes); }\n`;
    for (const name of ["rustc", "cargo", "npm"]) { await writeFile(join(tools, name), tool); await chmod(join(tools, name), 0o700); }
    const env = { ...process.env, PATH: `${tools}:${process.env.PATH}` };
    const invoke = (target) => spawnSync(process.execPath, [join(repositoryRoot, "scripts/build-headless.mjs"), "--target", target], { cwd: root, env, encoding: "utf8" });
    // The CLI uses its repository root. Import the same public builder with this disposable root instead.
    const result = spawnSync(process.execPath, ["--input-type=module", "-e", `import {buildHeadless} from ${JSON.stringify(new URL("./build-headless.mjs", import.meta.url).href)}; await buildHeadless({root:${JSON.stringify(root)}, target:'${linux}'});`], { cwd: root, env, encoding: "utf8" });
    assert.equal(result.status, 0, result.stderr);
    const verified = await verifyHeadlessDistribution({ directory: output, target: linux, packages });
    assert.deepEqual(verified.identities, packages.map(({ identity }) => identity));
    for (const path of sentinels.slice(0, -1)) assert.equal(await readFile(path, "utf8"), "preserve");
    await assert.rejects(readFile(sentinels.at(-1)), /ENOENT/);
    for (const [source, bytes] of sourceManifests) assert.deepEqual(await readFile(join(source, "teamarness.package.json")), bytes);
    const commands = (await readFile(join(root, "commands"), "utf8")).trim().split("\n").map(JSON.parse);
    const builds = commands.filter(([name, args]) => name === "cargo" && args[0] === "build").map(([, args]) => args);
    assert.deepEqual(builds.map((args) => args[args.indexOf("-p") + 1]), ["teamarness-headless", ...packages.map(({ cargoPackage }) => cargoPackage)]);
    for (const args of builds) assert.deepEqual(args.slice(-2), ["--target", linux]);
    assert.deepEqual(commands.filter(([name]) => name === "npm").map(([, args]) => args), [["--workspace", "@teamarness/package-ui-sdk", "run", "build"], ...packages.map(({ identity }) => ["--workspace", identity, "run", "build"])]);
    assert.doesNotMatch(JSON.stringify(commands), /tauri|desktop|codesign|external-gateway|dev.teamarness.agent/);
    const before = await readFile(join(output, "teamarness-headless"));
    const mismatch = invoke(mac);
    assert.notEqual(mismatch.status, 0);
    assert.match(mismatch.stderr, /not native/);
    assert.deepEqual(await readFile(join(output, "teamarness-headless")), before);
    await writeFile(join(output, "packages", "dev.teamarness.external-gateway"), "not bundled");
    await assert.rejects(verifyHeadlessDistribution({ directory: output, target: linux, packages }), /deep-equal/);
    await rm(join(output, "packages", "dev.teamarness.external-gateway"));
    await chmod(join(output, "teamarness-headless"), 0o755);
    await assert.rejects(verifyHeadlessDistribution({ directory: output, target: linux, packages }), /private regular executable/);
    await chmod(join(output, "teamarness-headless"), 0o700);
    const macBytes = Buffer.alloc(64); macBytes.writeUInt32LE(0xfeedfacf); macBytes.writeUInt32LE(0x0100000c, 4);
    await writeFile(join(output, "teamarness-headless"), macBytes);
    await assert.rejects(verifyHeadlessDistribution({ directory: output, target: linux, packages }), /do not match/);
    await writeFile(join(output, "teamarness-headless"), before);
    const backend = join(output, "packages", packages[0].identity, "teamarness.package.json");
    const manifest = JSON.parse(await readFile(backend));
    await writeFile(join(output, "packages", packages[0].identity, manifest.backends[0].path), "corrupt");
    await assert.rejects(verifyHeadlessDistribution({ directory: output, target: linux, packages }), /mismatch/);
  } finally { await rm(root, { recursive: true, force: true }); }
});
