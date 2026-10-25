// Package UI of the UI isolation probe. Hand written, no build step.
//
// It runs inside a Package UI Frame in the packaged application and reports, in
// text the packaged smoke can read through Accessibility, whether the frame can
// reach Tauri, the Shell realm, storage or the network, and whether the frame
// policy admits WebAssembly while still refusing other dynamic code (ADR 0031).
// It also attempts two real command posts through the WKWebView message
// handler; the smoke checks afterwards that neither of them created a Workspace.

const BREACH_FETCH = "ui-isolation-breach-fetch";
const BREACH_FAKE_KEY = "ui-isolation-breach-fake-key";
const BREACH_NO_KEY = "ui-isolation-breach-no-key";
const BREACH_TOP_NAVIGATION = "about:blank#ui-isolation-breach-top";

// The smallest valid WebAssembly module: the magic number and version 1.
const MINIMAL_WASM_MODULE = new Uint8Array([0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00]);

async function runCheck(name, check) {
  try {
    return [name, (await check()) === true];
  } catch {
    return [name, false];
  }
}

function throwsWhenRead(read) {
  try {
    read();
    return false;
  } catch {
    return true;
  }
}

async function wasmInstantiates() {
  const { instance } = await WebAssembly.instantiate(MINIMAL_WASM_MODULE);
  return instance instanceof WebAssembly.Instance;
}

function evalBlocked() {
  // Indirect eval, so a permissive policy would evaluate in the global scope.
  return throwsWhenRead(() => (0, eval)("1"));
}

function functionConstructorBlocked() {
  return throwsWhenRead(() => new Function("return 1"));
}

async function topNavigationBlocked() {
  try {
    window.top.location.href = BREACH_TOP_NAVIGATION;
  } catch {
    return true;
  }
  // WebKit may refuse without throwing. An admitted navigation replaces the
  // Shell document and this frame with it, so no verdict ever renders and the
  // smoke times out; a frame still running after the wait was not navigated away.
  await new Promise((resolve) => { setTimeout(resolve, 1_000); });
  return true;
}

async function blockedFetch() {
  const attempt = fetch("ipc://localhost/create_workspace", {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      "Tauri-Invoke-Key": "probe-fake-key",
      "Tauri-Callback": "1",
      "Tauri-Error": "2",
    },
    body: JSON.stringify({ name: BREACH_FETCH }),
  });
  const timeout = new Promise((resolve) => { setTimeout(() => resolve("blocked-by-timeout"), 2_000); });
  try {
    const outcome = await Promise.race([attempt, timeout]);
    return outcome === "blocked-by-timeout";
  } catch {
    return true;
  }
}

function attemptMessageHandlerPosts() {
  const handler = window.webkit?.messageHandlers?.ipc?.postMessage;
  if (typeof handler !== "function") return false;
  const post = (payload) => {
    try {
      window.webkit.messageHandlers.ipc.postMessage(JSON.stringify(payload));
    } catch {
      // A rejected post is still an attempt; the database check is the verdict.
    }
  };
  post({
    cmd: "create_workspace",
    callback: 1,
    error: 2,
    payload: { name: BREACH_FAKE_KEY },
    options: {},
    __TAURI_INVOKE_KEY__: "probe-fake-key",
  });
  post({
    cmd: "create_workspace",
    callback: 1,
    error: 2,
    payload: { name: BREACH_NO_KEY },
    options: {},
  });
  return true;
}

export async function mount(context) {
  const checks = [];
  // First, so an admitted top navigation tears the frame down before any verdict renders.
  checks.push(await runCheck("top-navigation-blocked", topNavigationBlocked));
  checks.push(await runCheck("no-tauri-internals", () => typeof window.__TAURI_INTERNALS__ === "undefined"));
  checks.push(await runCheck("no-tauri-global", () => typeof window.__TAURI__ === "undefined"));
  checks.push(await runCheck("no-window-ipc", () => typeof window.ipc === "undefined"));
  checks.push(await runCheck("parent-document-blocked", () => throwsWhenRead(() => window.parent.document)));
  checks.push(await runCheck("top-location-blocked", () => throwsWhenRead(() => window.top.location.href)));
  checks.push(await runCheck("opaque-origin", () => self.origin === "null"));
  checks.push(await runCheck("storage-blocked", () => throwsWhenRead(() => window.localStorage)));
  checks.push(await runCheck("ipc-fetch-blocked", blockedFetch));
  checks.push(await runCheck("wasm-instantiates", wasmInstantiates));
  checks.push(await runCheck("eval-blocked", evalBlocked));
  checks.push(await runCheck("function-constructor-blocked", functionConstructorBlocked));
  checks.push(await runCheck("theme-delivered", () =>
    getComputedStyle(document.documentElement).getPropertyValue("--teamarness-color-accent").trim() !== ""));
  checks.push(await runCheck("bridge-error-envelope", async () => {
    try {
      await context.client.request("probe.undeclared", {});
      return false;
    } catch (error) {
      return typeof error === "object" && error !== null && error.code === "ui_contract_violation";
    }
  }));

  const messageHandlerPresent = attemptMessageHandlerPosts();
  try {
    window.parent.postMessage({ kind: "request", id: 1, operation: "probe.forged", payload: {} }, "*");
  } catch {
    // The Shell installs no window listener, so a refused post changes nothing.
  }

  const list = document.createElement("ul");
  for (const [name, pass] of checks) {
    const item = document.createElement("li");
    item.textContent = `${name}: ${pass ? "pass" : "fail"}`;
    list.append(item);
  }
  const information = document.createElement("li");
  information.textContent = `messagehandler-present: ${messageHandlerPresent ? "true" : "false"}`;
  list.append(information);

  const verdict = document.createElement("p");
  verdict.textContent = checks.every(([, pass]) => pass) ? "Isolation probe PASS" : "Isolation probe FAIL";

  context.root.append(list, verdict);

  let removed = false;
  return () => {
    if (removed) return;
    removed = true;
    list.remove();
    verdict.remove();
  };
}
