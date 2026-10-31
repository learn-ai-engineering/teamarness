// Exhibits content isolation probe (test-only). It runs inside a `teamarness-content:` frame and
// reports, in visible text the packaged smoke reads through Accessibility, whether the content
// can reach the Package bridge, the Shell or Tauri. It also attempts real command posts through
// the WKWebView message handler and an `ipc:` fetch; the smoke then checks the Core database
// for Workspaces named `content-isolation-breach…`.
//
// The fragment names the frame: no fragment is the Exhibits viewer's sandboxed frame, and
// `#unsandboxed` is a frame a Package UI created without a `sandbox` attribute, which must be
// opaque-origin because the content scheme's CSP carries `sandbox allow-scripts`.
(async () => {
  const unsandboxed = location.hash === "#unsandboxed";
  const label = unsandboxed ? "Unsandboxed content frame probe" : "Content isolation probe";
  const suffix = unsandboxed ? "-unsandboxed" : "";
  document.getElementById("frame").textContent = label;

  const check = async (name, run) => {
    try { return [name, (await run()) === true]; } catch { return [name, false]; }
  };
  const throws = (read) => {
    try { read(); return false; } catch { return true; }
  };
  const settle = (ms) => new Promise((resolve) => { setTimeout(resolve, ms); });

  const checks = [];
  checks.push(await check("opaque-origin", () => self.origin === "null"));
  checks.push(await check("no-tauri-internals", () => typeof window.__TAURI_INTERNALS__ === "undefined"));
  checks.push(await check("no-tauri-global", () => typeof window.__TAURI__ === "undefined"));
  checks.push(await check("no-window-ipc", () => typeof window.ipc === "undefined"));
  checks.push(await check("parent-document-blocked", () => throws(() => window.parent.document)));
  checks.push(await check("top-document-blocked", () => throws(() => window.top.document)));
  checks.push(await check("top-location-blocked", () => throws(() => window.top.location.href)));
  checks.push(await check("storage-blocked", () => throws(() => window.localStorage)));
  checks.push(await check("ipc-fetch-blocked", async () => {
    const attempt = fetch("ipc://localhost/create_workspace", {
      method: "POST",
      headers: { "Content-Type": "application/json", "Tauri-Invoke-Key": "probe-fake-key", "Tauri-Callback": "1", "Tauri-Error": "2" },
      body: JSON.stringify({ name: `content-isolation-breach-fetch${suffix}` }),
    });
    const timeout = settle(2_000).then(() => "blocked-by-timeout");
    try { return (await Promise.race([attempt, timeout])) === "blocked-by-timeout"; } catch { return true; }
  }));
  checks.push(await check("forged-bridge-connect-ignored", async () => {
    let answered = false;
    for (const target of [window.parent, window.top]) {
      try {
        const channel = new MessageChannel();
        channel.port1.onmessage = () => { answered = true; };
        channel.port1.start();
        target.postMessage({ kind: "teamarness.bridge.connect", protocol: 2 }, "*", [channel.port2]);
      } catch {
        // A refused post is not a reply.
      }
    }
    await settle(1_500);
    return answered === false;
  }));

  const handler = window.webkit?.messageHandlers?.ipc;
  const handlerPresent = typeof handler?.postMessage === "function";
  if (handlerPresent) {
    for (const [name, key] of [[`content-isolation-breach-fake-key${suffix}`, "probe-fake-key"], [`content-isolation-breach-no-key${suffix}`, undefined]]) {
      try {
        handler.postMessage(JSON.stringify({
          cmd: "create_workspace", callback: 1, error: 2, payload: { name }, options: {},
          ...(key ? { __TAURI_INVOKE_KEY__: key } : {}),
        }));
      } catch {
        // A rejected post is still an attempt; the database check is the verdict.
      }
    }
  }

  const list = document.getElementById("checks");
  for (const [name, passed] of checks) {
    const item = document.createElement("li");
    item.textContent = `${passed ? "PASS" : "FAIL"} ${name}`;
    list.append(item);
  }
  const information = document.createElement("li");
  information.textContent = `INFO messagehandler-present ${handlerPresent}`;
  list.append(information);
  const passed = checks.every(([, ok]) => ok);
  document.getElementById("verdict").textContent = `${label} ${passed ? "PASS" : "FAIL"}`;
})();
