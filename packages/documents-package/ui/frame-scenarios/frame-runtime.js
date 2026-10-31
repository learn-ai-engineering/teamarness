// The frame-scenarios stand-in for the Shell's frame runtime. It runs inside the Shell's own
// frame document under the frame policy, mounts the built Documents UI (`index.js`) into an open
// shadow root, and carries its Package client over postMessage to the parent page, which relays
// to the harness Backends. It holds no editor logic of its own.
const view = new URLSearchParams(location.search).get("view");
const toParent = (message) => parent.postMessage({ view, ...message }, "*");

document.addEventListener("securitypolicyviolation", (event) => {
  toParent({ kind: "csp", violation: { directive: event.effectiveDirective, blocked: event.blockedURI, sample: event.sample, source: event.sourceFile, line: event.lineNumber, column: event.columnNumber } });
}, true);
window.addEventListener("error", (event) => toParent({ kind: "error", error: String(event.message) }));
window.addEventListener("unhandledrejection", (event) => toParent({ kind: "error", error: String(event.reason?.stack ?? event.reason) }));
// The copy command's text, as the copy event carries it once every other listener has run.
window.addEventListener("copy", (event) => {
  toParent({ kind: "copied", text: event.clipboardData?.getData("text/plain") ?? null });
});

const THEME = `:root {
  --teamarness-color-background: #ffffff; --teamarness-color-panel: #f6f6f4; --teamarness-color-text: #1d1d1b;
  --teamarness-color-text-muted: #66665f; --teamarness-color-border: #d9d9d3; --teamarness-color-accent: #2f6fde;
  --teamarness-color-danger: #b42318; --teamarness-color-focus: #2f6fde; --teamarness-font-body: system-ui, sans-serif;
  --teamarness-font-mono: ui-monospace, monospace; --teamarness-radius-control: 6px; --teamarness-radius-panel: 8px;
}`;

let nextId = 0;
const pending = new Map();
const listeners = new Map();

function request(operation, payload) {
  const id = ++nextId;
  return new Promise((resolve, reject) => {
    pending.set(id, { resolve, reject });
    toParent({ kind: "request", id, operation, payload });
  });
}

const client = {
  request: (operation, payload) => request(operation, payload),
  async subscribe(operation, payload, listener, options) {
    const id = ++nextId;
    listeners.set(id, { listener, options });
    try {
      await new Promise((resolve, reject) => {
        pending.set(id, { resolve, reject });
        toParent({ kind: "subscribe", id, operation, payload });
      });
    } catch (error) {
      listeners.delete(id);
      throw error;
    }
    return {
      close: async () => {
        if (!listeners.delete(id)) return;
        toParent({ kind: "close", id });
      },
    };
  },
};

window.addEventListener("message", (event) => {
  if (event.source !== parent) return;
  const message = event.data;
  if (message.kind === "response") {
    const waiter = pending.get(message.id);
    pending.delete(message.id);
    if (message.ok) waiter?.resolve(message.value);
    else waiter?.reject(message.error);
  } else if (message.kind === "event") {
    void listeners.get(message.id)?.listener(message.event);
  } else if (message.kind === "terminated") {
    const entry = listeners.get(message.id);
    listeners.delete(message.id);
    entry?.options?.onTerminated?.(message.error ?? null);
  } else if (message.kind === "mount") {
    void mount(message);
  }
});

async function mount(message) {
  try {
    const sheet = new CSSStyleSheet();
    sheet.replaceSync(THEME);
    document.adoptedStyleSheets = [sheet];
    const host = document.getElementById("teamarness-package-host");
    const root = host.attachShadow({ mode: "open" });
    const module = await import("./index.js");
    await module.mount(Object.freeze({
      root,
      package: Object.freeze({
        identity: "dev.teamarness.documents", version: "1.0.0", displayName: message.displayName,
        source: "bundled", trust: "application_bundled", lifecycle: "active", processGeneration: "1",
      }),
      workspace: Object.freeze({ id: "frame-scenarios", name: "Frame scenarios" }),
      uiHostVersion: 1,
      themeTokens: [],
      client,
    }));
    toParent({ kind: "mounted" });
  } catch (error) {
    toParent({ kind: "fault", error: `${error?.name}: ${error?.message}` });
  }
}

toParent({ kind: "hello" });
