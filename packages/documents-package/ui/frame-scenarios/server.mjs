// The frame-scenarios static server (node:http only).
//
// It serves the Shell-owned frame document, its stylesheet and its Content Security Policy as
// the desktop app does, read from the Shell's own source (`package_protocol.rs`), never copied.
// The only change is the origin: the desktop serves frames from `teamarness-package:`, and this
// server is `http://127.0.0.1:<port>`, so that scheme source becomes this origin.
import http from "node:http";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const ui = path.resolve(here, "..");
const repository = path.resolve(ui, "../../..");
export const PACKAGE_PROTOCOL = path.join(repository, "apps/teamarness-desktop/src-tauri/src/package_protocol.rs");

/** One `const NAME: &str = "...";` or raw `r#"..."#` string out of the Rust source. */
export function rustString(source, name) {
  const raw = new RegExp(`const ${name}: &str = r#"([\\s\\S]*?)"#;`).exec(source);
  if (raw) return raw[1];
  const plain = new RegExp(`const ${name}: &str = "((?:[^"\\\\]|\\\\.)*)";`).exec(source);
  if (plain) return plain[1].replace(/\\(.)/g, "$1");
  throw new Error(`${name} is not a string constant in package_protocol.rs`);
}

/** The frame resources exactly as the Shell defines them. */
export function shellFrame() {
  const source = fs.readFileSync(PACKAGE_PROTOCOL, "utf8");
  return {
    csp: rustString(source, "FRAME_DOCUMENT_CSP"),
    document: rustString(source, "FRAME_DOCUMENT"),
    stylesheet: rustString(source, "FRAME_STYLESHEET"),
  };
}

/** The frame policy with the Package scheme source replaced by `origin`. */
export function framePolicy(csp, origin) {
  if (!csp.includes("teamarness-package:")) throw new Error("FRAME_DOCUMENT_CSP no longer names teamarness-package:");
  return csp.replaceAll("teamarness-package:", origin);
}

export function startServer({ port = 0 } = {}) {
  const frame = shellFrame();
  const files = {
    "/": [path.join(here, "parent.html"), "text/html; charset=utf-8"],
    "/parent.js": [path.join(here, "parent.js"), "text/javascript; charset=utf-8"],
    "/frame-runtime.js": [path.join(here, "frame-runtime.js"), "text/javascript; charset=utf-8"],
    "/index.js": [path.join(ui, "dist/index.js"), "text/javascript; charset=utf-8"],
    "/style.css": [path.join(ui, "dist/style.css"), "text/css; charset=utf-8"],
    "/probe.txt": [path.join(here, "server.mjs"), "text/plain; charset=utf-8"],
  };
  const server = http.createServer((request, response) => {
    const url = new URL(request.url ?? "/", "http://localhost");
    const origin = `http://${request.headers.host}`;
    const headers = {
      "x-content-type-options": "nosniff",
      "cache-control": "no-store",
      // A sandboxed frame has an opaque origin, so its module scripts are CORS requests.
      "access-control-allow-origin": "*",
    };
    let body;
    if (url.pathname === "/frame.html") {
      body = Buffer.from(frame.document);
      headers["content-type"] = "text/html; charset=utf-8";
    } else if (url.pathname === "/frame.css") {
      body = Buffer.from(frame.stylesheet);
      headers["content-type"] = "text/css; charset=utf-8";
    } else if (files[url.pathname]) {
      const [file, type] = files[url.pathname];
      body = fs.readFileSync(file);
      headers["content-type"] = type;
    } else {
      response.writeHead(404, headers);
      response.end();
      return;
    }
    if (url.pathname === "/probe.txt") server.probeHits = (server.probeHits ?? 0) + 1;
    // Every resource a frame loads carries the frame policy, as the desktop protocol does.
    if (url.pathname !== "/" && url.pathname !== "/parent.js") headers["content-security-policy"] = framePolicy(frame.csp, origin);
    response.writeHead(200, headers);
    response.end(body);
  });
  return new Promise((resolve) => server.listen(port, "127.0.0.1", () => resolve(server)));
}
