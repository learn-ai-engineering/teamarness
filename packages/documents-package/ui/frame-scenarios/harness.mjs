// One scenario's world: a browser page playing the Shell, one sandboxed frame per view, and a
// Workspace of harness Backends in Node. Scenarios drive the frames with real keyboard and
// pointer input through Playwright, and read results back from the frames' DOM and from the
// Rust Markdown authority.
import { DOCUMENT, Workspace } from "./backend.mjs";

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

export class Harness {
  static async open(browser, origin, authority) {
    const context = await browser.newContext({ viewport: { width: 1180, height: 1400 } });
    const page = await context.newPage();
    const harness = new Harness(context, page, new Workspace(authority));
    await page.exposeFunction("__harness", (view, message) => harness.receive(view, message));
    page.on("console", (message) => {
      if (message.type() === "error" || message.type() === "warning") {
        harness.console.push({ type: message.type(), text: message.text().slice(0, 400), url: message.location()?.url ?? "" });
      }
    });
    page.on("pageerror", (error) => harness.console.push({ type: "pageerror", text: String(error).slice(0, 400) }));
    await page.goto(`${origin}/`);
    return harness;
  }

  constructor(context, page, workspace) {
    this.context = context;
    this.page = page;
    this.workspace = workspace;
    this.views = new Map();
    this.console = [];
    this.csp = [];
    this.errors = [];
    this.copied = [];
    this.delivery = Promise.resolve();
    workspace.deliver = (view, message) => {
      this.delivery = this.delivery
        .then(() => page.evaluate(([target, payload]) => window.__deliver(target, payload), [view, message]))
        .catch((error) => { if (!page.isClosed()) this.errors.push({ view, error: String(error) }); });
    };
  }

  async receive(view, message) {
    switch (message.kind) {
      case "csp": this.csp.push({ view, ...message.violation }); return null;
      case "error": this.errors.push({ view, error: message.error }); return null;
      case "fault": this.errors.push({ view, error: `mount fault: ${message.error}` }); return null;
      case "copied": this.copied.push({ view, text: message.text }); return null;
      default: {
        const installation = this.views.get(view);
        if (installation === undefined) return null;
        return this.workspace.installation(installation).handle(view, message);
      }
    }
  }

  /** Mounts the Documents UI for `view` on `installation` and opens the scenario Document. */
  async addView(view, installation) {
    this.views.set(view, installation);
    this.workspace.installation(installation);
    const mounted = await this.page.evaluate((name) => window.__addView(name), view);
    if (mounted.kind !== "mounted") throw new Error(`${view} did not mount: ${mounted.error}`);
    const frame = this.frame(view);
    await frame.locator(".document-title").first().click();
    await frame.locator(".ProseMirror").waitFor({ state: "visible", timeout: 10_000 });
    await this.waitFor(async () => (await this.editor(view).getAttribute("contenteditable")) === "true", `${view} editable`);
  }

  frame(view) {
    return this.page.frameLocator(`iframe[title="${view}"]`);
  }

  /** The Playwright frame of `view`, for reading state from inside it. */
  rawFrame(view) {
    return this.page.frames().find((frame) => frame.url().includes(`view=${encodeURIComponent(view)}`));
  }

  editor(view) {
    return this.frame(view).locator(".ProseMirror");
  }

  async setOnline(installation, online) {
    this.workspace.setOnline(installation, online);
    await this.delivery;
  }

  /** The rendered body of `view` as structure and text, without ProseMirror's own bookkeeping. */
  async rendered(view) {
    return this.rawFrame(view).evaluate(() => {
      const root = document.getElementById("teamarness-package-host").shadowRoot.querySelector(".ProseMirror");
      const walk = (node) => {
        if (node.nodeType === Node.TEXT_NODE) return node.data;
        if (node.nodeType !== Node.ELEMENT_NODE) return "";
        if (node.classList.contains("ProseMirror-trailingBreak") || node.classList.contains("ProseMirror-separator")) return "";
        const tag = node.tagName.toLowerCase();
        const attributes = ["href", "start"].filter((name) => node.hasAttribute(name)).map((name) => ` ${name}="${node.getAttribute(name)}"`).join("");
        return `<${tag}${attributes}>${[...node.childNodes].map(walk).join("")}</${tag}>`;
      };
      return root ? [...root.childNodes].map(walk).join("") : null;
    });
  }

  /** The visible text of the whole mount of `view`. */
  async visibleText(view) {
    return this.rawFrame(view).evaluate(() => document.getElementById("teamarness-package-host").shadowRoot.querySelector(".documents-page").innerText);
  }

  markdown(installation) {
    return this.workspace.installation(installation).markdown();
  }

  /**
   * Waits until every view renders the same body and every Installation's Backend exports the
   * same Canonical Markdown. Returns that Markdown.
   */
  async converge(views, timeout = 15_000) {
    const installations = [...new Set(views.map((view) => this.views.get(view)))];
    const deadline = Date.now() + timeout;
    let last;
    for (;;) {
      await this.delivery;
      const rendered = await Promise.all(views.map((view) => this.rendered(view)));
      const markdown = installations.map((installation) => this.markdown(installation));
      last = { rendered, markdown };
      if (rendered.every((body) => body === rendered[0]) && markdown.every((text) => text === markdown[0])) {
        // The views must also hold what their Backends hold: settle once more after the send cadence.
        await sleep(400);
        const again = await Promise.all(views.map((view) => this.rendered(view)));
        const exports = installations.map((installation) => this.markdown(installation));
        if (again.every((body) => body === rendered[0]) && exports.every((text) => text === markdown[0])) return markdown[0];
      }
      if (process.env.FRAME_SCENARIOS_DEBUG) console.log("converge: waiting", JSON.stringify(last).slice(0, 300));
      if (Date.now() > deadline) throw new Error(`views did not converge: ${JSON.stringify(last).slice(0, 1500)}`);
      await sleep(150);
    }
  }

  async waitFor(check, label, timeout = 10_000) {
    const deadline = Date.now() + timeout;
    for (;;) {
      if (await check()) return;
      if (Date.now() > deadline) throw new Error(`timed out waiting for ${label}`);
      await sleep(100);
    }
  }

  /** What the system clipboard holds, pasted with the keyboard into the Shell page's own field. */
  async pasteClipboard() {
    const field = this.page.locator("#clipboard");
    await field.fill("");
    await field.focus();
    await this.page.keyboard.press("ControlOrMeta+v");
    return field.inputValue();
  }

  /** Waits for the views' send cadence, so their edits have reached the Backends. */
  async settle(ms = 700) {
    await sleep(ms);
    await this.delivery;
  }

  close() {
    return this.context.close();
  }
}

export { DOCUMENT, sleep };
