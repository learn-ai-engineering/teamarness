// The rendered-editor frame scenarios. Each runs in a fresh page, with views mounted in
// sandboxed frames under the frame policy, and drives them with real keyboard and pointer input.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { MAX_UPDATE } from "./backend.mjs";
import { sleep } from "./harness.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const ALL_NODES = fs.readFileSync(path.resolve(here, "../../fixtures/body-trees/from-rust/01-all-nodes.md"), "utf8");
const MOD = "ControlOrMeta";
/** As a person would, lets the editor see a selection before acting on it. */
const SELECTION_SETTLE_MS = 80;
/** macOS moves to a line's ends with Cmd+Arrow; Home and End only scroll there. */
const mac = process.platform === "darwin";
const LINE_START = mac ? "Meta+ArrowLeft" : "Home";
const LINE_END = mac ? "Meta+ArrowRight" : "End";
const DOCUMENT_END = mac ? "Meta+ArrowDown" : "Control+End";
/** The undo manager merges a peer's edits made within a second of each other into one step. */
const UNDO_MERGE_MS = 1100;

async function type(h, text) {
  await h.page.keyboard.type(text, { delay: 5 });
}

async function press(h, key, times = 1) {
  for (let index = 0; index < times; index += 1) await h.page.keyboard.press(key);
  // A person never moves the caret and acts in the same instant: let the editor see where it went.
  if (/Arrow|Home|End/.test(key)) await sleep(SELECTION_SETTLE_MS);
}


/** Selects the previous `count` characters with Shift+ArrowLeft. */
async function selectBack(h, count) {
  await press(h, "Shift+ArrowLeft", count);
}

/** Selects the next `count` characters with Shift+ArrowRight. */
async function selectForward(h, count) {
  await press(h, "Shift+ArrowRight", count);
}

/** Types `word`, applies a mark with `chord` to it, and types on unformatted. */
async function markWord(h, word, chord) {
  await type(h, word);
  await selectBack(h, word.length);
  await press(h, chord);
  await press(h, LINE_END);
  await press(h, chord);
  await type(h, " ");
}

/** Puts the caret in the `index`th top-level block of `view`, at its start or end. */
async function caret(h, view, index, where = "end") {
  await h.editor(view).locator(":scope > *").nth(index).click();
  await press(h, where === "end" ? LINE_END : LINE_START);
}

/** Dispatches a real paste event, as the browser does, carrying `data` on its clipboard. */
async function paste(h, view, data) {
  await h.rawFrame(view).evaluate((items) => {
    const body = document.getElementById("teamarness-package-host").shadowRoot.querySelector(".ProseMirror");
    const transfer = new DataTransfer();
    for (const [type, value] of Object.entries(items)) transfer.setData(type, value);
    body.dispatchEvent(new ClipboardEvent("paste", { clipboardData: transfer, bubbles: true, cancelable: true }));
  }, data);
}

function authored(h, installation) {
  return h.workspace.records.filter((record) => record.author === installation);
}

/** Markdown syntax that must never appear in what a Member sees while editing. */
const MARKDOWN_SYNTAX = [/\*\*/, /~~/, /```/, /\]\(/, /^#{1,6} /m, /^> /m, /^- /m, /^\d+\. /m, /^-{3,}$/m, /`[^`]+`/];

async function assertNoMarkdown(h, view) {
  const visible = await h.visibleText(view);
  for (const pattern of MARKDOWN_SYNTAX) {
    const match = pattern.exec(visible);
    assert.equal(match, null, `${view} shows Markdown ${pattern}: ${JSON.stringify(visible.slice(Math.max(0, (match?.index ?? 0) - 80), (match?.index ?? 0) + 80))}`);
  }
  const fields = await h.rawFrame(view).evaluate(() => [...document.getElementById("teamarness-package-host").shadowRoot.querySelectorAll("textarea, input")]
    .filter((field) => field.offsetParent !== null).map((field) => field.value));
  for (const value of fields) assert.equal(value, "", `${view} shows text in a visible field`);
}

const EXPECTED_ALL_NODES = [
  "# Heading one",
  "",
  "###### Six",
  "",
  "## Two",
  "",
  "Plain **bold** *italic* ~~strike~~ `code` **keys** *slant* ~~gone~~ `mono` [site](https://example.com/a) end\\",
  "after break",
  "",
  "> quoted",
  "",
  "- bullet a",
  "- bullet b",
  "  - nested",
  "",
  "<!-- end list -->",
  "",
  "1. first",
  "2. second",
  "",
  "<!-- end list -->",
  "",
  "- shortcut item",
  "",
  "> quoted by key",
  "",
  "-----",
  "",
  "between",
  "",
  "-----",
  "",
  "```rust",
  "fn main() {}",
  "```",
  "",
  "```",
  "plain code",
  "```",
  "",
  "```",
  "shortcut code",
  "```",
  "",
  "back to text",
  "",
  "Last line.",
  "",
].join("\n");

export const SCENARIOS = [
  {
    name: "every node and mark through real keyboard input",
    async run(h) {
      await h.addView("A1", "A");
      await h.addView("B1", "B");
      await h.editor("A1").click();
      await type(h, "# Heading one");
      await press(h, "Enter");
      await type(h, "###### Six");
      await press(h, "Enter");
      await type(h, "Two");
      await press(h, "Control+Shift+Digit2");
      await press(h, "Enter");
      await type(h, "Plain **bold** *italic* ~~strike~~ `code` ");
      await markWord(h, "keys", `${MOD}+b`);
      await markWord(h, "slant", `${MOD}+i`);
      await markWord(h, "gone", `${MOD}+Shift+KeyS`);
      await markWord(h, "mono", `${MOD}+e`);
      await type(h, "site");
      await selectBack(h, 4);
      await press(h, `${MOD}+k`);
      const prompt = h.frame("A1").locator(".editor-link-prompt input");
      await prompt.waitFor({ state: "visible" });
      await type(h, "https://example.com/a");
      await press(h, "Enter");
      await prompt.waitFor({ state: "hidden" });
      await press(h, LINE_END);
      await type(h, " end");
      await press(h, "Shift+Enter");
      await type(h, "after break");
      await press(h, "Enter");
      await type(h, "> quoted");
      await press(h, "Enter", 2);
      await type(h, "- bullet a");
      await press(h, "Enter");
      await type(h, "bullet b");
      await press(h, "Enter");
      await press(h, `${MOD}+BracketRight`);
      await type(h, "nested");
      await press(h, "Enter", 3);
      await type(h, "1. first");
      await press(h, "Enter");
      await type(h, "second");
      await press(h, "Enter", 2);
      await type(h, "shortcut item");
      await press(h, "Control+Shift+Digit8");
      await press(h, "Enter", 2);
      await type(h, "quoted by key");
      await press(h, "Control+Shift+Period");
      await press(h, "Enter", 2);
      await type(h, "---");
      await type(h, "between");
      await press(h, `${MOD}+Shift+Minus`);
      await type(h, "```rust");
      await press(h, "Enter");
      await type(h, "fn main() {}");
      await press(h, `${MOD}+Enter`);
      await type(h, "``` plain code");
      await press(h, `${MOD}+Enter`);
      await type(h, "shortcut code");
      await press(h, "Control+Shift+Backslash");
      await press(h, `${MOD}+Enter`);
      await type(h, "back to text");
      await press(h, "Control+Shift+Digit1");
      await press(h, "Control+Shift+Digit0");
      await press(h, "Enter");
      await type(h, "Last line.");
      const markdown = await h.converge(["A1", "B1"]);
      assert.equal(markdown, EXPECTED_ALL_NODES);
      const rendered = await h.rendered("B1");
      for (const tag of ["h1", "h2", "h6", "strong", "em", "s", "code", "a", "br", "blockquote", "ul", "ol", "li", "hr", "pre"]) {
        assert.match(rendered, new RegExp(`<${tag}[ >]`), `B1 renders <${tag}>`);
      }
      await assertNoMarkdown(h, "A1");
      await assertNoMarkdown(h, "B1");
      return `${markdown.length} bytes of Canonical Markdown, identical on both Installations`;
    },
  },
  {
    name: "no Markdown visible",
    async run(h) {
      await h.addView("A1", "A");
      await h.addView("B1", "B");
      h.workspace.installation("A").revise(ALL_NODES);
      const markdown = await h.converge(["A1", "B1"]);
      assert.equal(markdown, ALL_NODES);
      const rendered = await h.rendered("A1");
      for (const tag of ["h1", "h2", "h3", "h4", "h5", "h6", "strong", "em", "s", "code", "a", "br", "blockquote", "ul", "ol", "hr", "pre"]) {
        assert.match(rendered, new RegExp(`<${tag}[ >]`), `A1 renders <${tag}>`);
      }
      await assertNoMarkdown(h, "A1");
      await assertNoMarkdown(h, "B1");
      // Editing the rendered body keeps it rendered.
      await caret(h, "A1", 6);
      await type(h, " typed");
      await h.converge(["A1", "B1"]);
      await assertNoMarkdown(h, "A1");
      await assertNoMarkdown(h, "B1");
      return "every node and mark rendered; no Markdown syntax in either frame";
    },
  },
  {
    name: "same-paragraph concurrent edits with overlapping formatting",
    async run(h) {
      await h.addView("A1", "A");
      await h.addView("B1", "B");
      await h.editor("A1").click();
      await type(h, "The quick brown fox jumps over the lazy dog");
      await h.converge(["A1", "B1"]);
      await h.setOnline("A", false);
      await h.setOnline("B", false);
      await caret(h, "A1", 0, "start");
      await press(h, "ArrowRight", 4);
      await selectForward(h, 11);
      await press(h, `${MOD}+b`);
      await press(h, LINE_START);
      await press(h, "ArrowRight", 19);
      await type(h, " AAA");
      await caret(h, "B1", 0, "start");
      await press(h, "ArrowRight", 10);
      await selectForward(h, 15);
      await press(h, `${MOD}+i`);
      await press(h, LINE_START);
      await press(h, "ArrowRight", 19);
      await type(h, " BBB");
      await press(h, LINE_END);
      await press(h, "ArrowLeft", 3);
      await press(h, "Backspace", 5);
      await h.settle();
      assert.doesNotMatch(await h.rendered("A1"), /BBB/);
      assert.doesNotMatch(await h.rendered("B1"), /AAA/);
      await h.setOnline("A", true);
      await h.setOnline("B", true);
      const markdown = await h.converge(["A1", "B1"]);
      const text = (await h.rendered("A1")).replace(/<[^>]+>/g, "");
      for (const part of ["The quick brown fox", "AAA", "BBB", "jumps over the dog"]) assert.ok(text.includes(part), `${part} in ${text}`);
      assert.ok(!text.includes("lazy"), text);
      const rendered = await h.rendered("A1");
      assert.match(rendered, /<strong>[^<]*<em>brown|<em>[^<]*<strong>brown|<strong><em>[^<]*brown|<em><strong>[^<]*brown/, rendered);
      return JSON.stringify(markdown);
    },
  },
  {
    name: "offline-then-online convergence",
    async run(h) {
      await h.addView("A1", "A");
      await h.addView("B1", "B");
      await h.editor("A1").click();
      await type(h, "para one");
      await press(h, "Enter");
      await type(h, "para two");
      await press(h, "Enter");
      await type(h, "para three");
      await h.converge(["A1", "B1"]);
      await h.setOnline("B", false);
      await caret(h, "A1", 0);
      await type(h, " A1");
      await caret(h, "A1", 1);
      await type(h, " A2");
      await caret(h, "B1", 1);
      await type(h, " B2");
      await caret(h, "B1", 2);
      await type(h, " B3");
      await press(h, "Control+Shift+Digit2");
      await h.settle();
      assert.doesNotMatch(await h.rendered("A1"), /B2|B3/);
      assert.doesNotMatch(await h.rendered("B1"), /A1|A2/);
      assert.doesNotMatch(h.markdown("A"), /B2/);
      assert.ok(authored(h, "B").length > 0, "B appended its edits locally while offline");
      await h.setOnline("B", true);
      const markdown = await h.converge(["A1", "B1"]);
      // Concurrent inserts at one place are ordered by peer, which is random per session.
      assert.match(markdown, /^para one A1\n\npara two (A2 B2|B2 A2)\n\n## para three B3\n$/);
      return JSON.stringify(markdown);
    },
  },
  {
    name: "two views on one Installation",
    async run(h) {
      await h.addView("A1", "A");
      await h.addView("A2", "A");
      await h.addView("B1", "B");
      await h.editor("A1").click();
      await type(h, "shared start");
      await h.converge(["A1", "A2", "B1"]);
      const sessions = [...h.workspace.installation("A").sessions.keys()];
      assert.equal(new Set(sessions).size, 2, "each view holds its own session peer");
      await caret(h, "A1", 0);
      await type(h, " v1");
      await caret(h, "A2", 0, "start");
      await type(h, "v2 ");
      await caret(h, "A1", 0);
      await type(h, " more");
      const markdown = await h.converge(["A1", "A2", "B1"]);
      assert.equal(markdown, "v2 shared start v1 more\n");
      const peers = new Set();
      for (const [peer] of h.workspace.installation("A").body.oplogVersion().toJSON()) peers.add(peer);
      for (const peer of sessions) assert.ok(peers.has(peer), `session peer ${peer} edited`);
      return `${authored(h, "A").length} records from two session peers; ${JSON.stringify(markdown)}`;
    },
  },
  {
    name: "per-person undo with a remote edit and a port-style revise interleaved",
    async run(h) {
      await h.addView("A1", "A");
      await h.addView("B1", "B");
      await h.editor("A1").click();
      await type(h, "base text");
      await h.converge(["A1", "B1"]);
      await sleep(UNDO_MERGE_MS);
      await caret(h, "A1", 0, "start");
      await type(h, "mine ");
      await h.settle(UNDO_MERGE_MS);
      await caret(h, "B1", 0);
      await type(h, " theirs");
      await h.converge(["A1", "B1"]);
      const current = h.markdown("A");
      h.workspace.installation("A").revise(`${current}\nAn agent paragraph.\n`);
      await h.converge(["A1", "B1"]);
      await h.editor("A1").focus();
      await press(h, `${MOD}+z`);
      const undone = await h.converge(["A1", "B1"]);
      assert.equal(undone, "base text theirs\n\nAn agent paragraph.\n");
      await press(h, `${MOD}+Shift+KeyZ`);
      const redone = await h.converge(["A1", "B1"]);
      assert.equal(redone, "mine base text theirs\n\nAn agent paragraph.\n");
      await press(h, `${MOD}+z`);
      await press(h, `${MOD}+y`);
      assert.equal(await h.converge(["A1", "B1"]), redone);
      return JSON.stringify(undone);
    },
  },
  {
    name: "a 150 KB paste splits and converges",
    pastesStyledHtml: true,
    async run(h) {
      await h.addView("A1", "A");
      await h.addView("B1", "B");
      await h.editor("A1").click();
      await type(h, "Before the paste.");
      await h.converge(["A1", "B1"]);
      const before = h.workspace.records.length;
      const paragraphs = Array.from({ length: 610 }, (_, index) => `Paragraph ${index} ${"lorem ipsum dolor sit amet ".repeat(9)}`.slice(0, 250).trim());
      const html = paragraphs.map((text, index) => index % 50 === 0 ? `<p style="color: red">${text}</p>` : `<p>${text}</p>`).join("");
      await paste(h, "A1", { "text/html": html, "text/plain": paragraphs.join("\n\n") });
      const markdown = await h.converge(["A1", "B1"], 60_000);
      const added = h.workspace.records.slice(before);
      assert.ok(added.length >= 3, `the paste became ${added.length} records`);
      const largest = Math.max(...added.map((record) => record.bytes.byteLength));
      assert.ok(largest <= MAX_UPDATE, `largest record ${largest} bytes`);
      assert.deepEqual(h.workspace.refusals, []);
      assert.ok(markdown.includes(paragraphs[0]) && markdown.includes(paragraphs.at(-1)), "both ends of the paste arrived");
      const pasted = paragraphs.reduce((total, text) => total + text.length, 0);
      assert.ok(pasted >= 150_000, `${pasted} bytes pasted`);
      return `${pasted} bytes pasted as ${added.length} records, largest ${largest} bytes; UI payload peak ${h.workspace.largestPayload} bytes`;
    },
  },
  {
    name: "an over-bound single block is refused with nothing committed",
    async run(h) {
      await h.addView("A1", "A");
      await h.addView("B1", "B");
      await h.editor("A1").click();
      await type(h, "keep me");
      await h.converge(["A1", "B1"]);
      const before = h.workspace.records.length;
      const renderedBefore = await h.rendered("A1");
      await paste(h, "A1", { "text/plain": "x".repeat(170_000) });
      const notice = h.frame("A1").locator(".editor-notice");
      await notice.waitFor({ state: "visible" });
      assert.match(await notice.innerText(), /too large/);
      await h.settle(1000);
      assert.equal(h.workspace.records.length, before, "no record was appended");
      assert.equal(await h.rendered("A1"), renderedBefore);
      assert.equal(await h.converge(["A1", "B1"]), "keep me\n");
      assert.deepEqual(h.workspace.refusals, []);
      // The editor is still usable afterwards.
      await caret(h, "A1", 0);
      await type(h, " and this");
      assert.equal(await h.converge(["A1", "B1"]), "keep me and this\n");
      return "refused before commit; the Backends saw nothing";
    },
  },
  {
    name: "copy as Markdown matches the Rust export",
    async run(h, { engine }) {
      await h.addView("A1", "A");
      await h.addView("B1", "B");
      h.workspace.installation("B").revise(ALL_NODES);
      await h.converge(["A1", "B1"]);
      await caret(h, "A1", 0);
      await type(h, " (edited)");
      // A long tail makes the export span several 24 KiB chunks.
      h.workspace.installation("B").revise(`${h.markdown("B")}\n${"A long paragraph of text. ".repeat(4000)}\n`);
      const expected = await h.converge(["A1", "B1"]);
      assert.ok(Buffer.byteLength(expected) > 3 * 24 * 1024);
      if (engine === "chromium") await h.context.grantPermissions(["clipboard-read", "clipboard-write"], { origin: new URL(h.page.url()).origin });
      await h.frame("A1").locator(".editor-copy-markdown").click();
      const status = h.frame("A1").locator(".editor-copy-status");
      const panel = h.frame("A1").locator(".markdown-copy");
      await h.waitFor(async () => (await status.innerText()) === "Copied as Markdown" || await panel.isVisible(), "the copy outcome");
      let copied;
      let path;
      if (await panel.isVisible()) {
        path = "visible fallback";
        assert.equal(await panel.locator("textarea").inputValue(), expected);
        await panel.getByRole("button", { name: "Copy", exact: true }).click();
        await h.waitFor(async () => h.copied.length > 0, "the copy command");
        copied = h.copied.at(-1).text;
      } else {
        path = "clipboard";
        copied = await h.pasteClipboard();
      }
      assert.equal(copied, expected);
      const markdownRequests = h.workspace.installation("A").markdownCache?.chunks.length;
      return `${path}: ${Buffer.byteLength(expected)} bytes in ${markdownRequests} chunks, equal to the Rust export`;
    },
  },
  {
    name: "comment on selection",
    async run(h) {
      await h.addView("A1", "A");
      await h.editor("A1").click();
      await type(h, "Alpha bravo charlie");
      await h.settle();
      await caret(h, "A1", 0, "start");
      await press(h, "ArrowRight", 6);
      await selectForward(h, 5);
      await h.frame("A1").locator(".editor-comment").click();
      const draft = h.frame("A1").locator(".comment-quote--draft .comment-quote-excerpt");
      await draft.waitFor({ state: "visible" });
      assert.equal(await draft.innerText(), "bravo");
      await type(h, "Nice word");
      await h.frame("A1").locator(".comment-compose button[type=submit]").click();
      await h.waitFor(async () => h.workspace.comments.length === 1, "the Comment");
      assert.equal(h.workspace.comments[0].quote, "bravo");
      assert.equal(h.workspace.comments[0].markdown, "Nice word");
      await h.frame("A1").locator(".comment .comment-quote").first().waitFor({ state: "visible" });
      assert.equal(await h.frame("A1").locator(".comment .comment-quote").first().innerText(), "bravo");
      // A long selection is quoted up to 2 KiB.
      h.workspace.installation("A").revise(`Alpha bravo charlie\n\n${"é".repeat(3000)}\n`);
      await h.converge(["A1"]);
      await h.editor("A1").click();
      await press(h, `${MOD}+a`);
      await sleep(SELECTION_SETTLE_MS);
      await h.frame("A1").locator(".comment-input").click();
      await type(h, "All of it");
      await h.frame("A1").locator(".comment-compose button[type=submit]").click();
      await h.waitFor(async () => h.workspace.comments.length === 2, "the second Comment");
      const quote = h.workspace.comments[1].quote;
      assert.ok(Buffer.byteLength(quote) <= 2048 && Buffer.byteLength(quote) > 2000, `quote of ${Buffer.byteLength(quote)} bytes`);
      assert.ok(quote.startsWith("Alpha bravo charlie\né"), quote.slice(0, 40));
      return `quotes "bravo" and ${Buffer.byteLength(quote)} bytes of a long selection`;
    },
  },
  {
    name: "no console errors in an editing session",
    async run(h) {
      await h.addView("A1", "A");
      await h.addView("A2", "A");
      await h.addView("B1", "B");
      h.workspace.installation("A").revise(ALL_NODES);
      await h.converge(["A1", "A2", "B1"]);
      await h.editor("B1").click();
      await press(h, DOCUMENT_END);
      await type(h, "More text **bold** done");
      await press(h, `${MOD}+z`);
      await h.converge(["A1", "A2", "B1"]);
      // run.mjs fails any scenario that leaves console errors, policy reports or frame errors.
      return `console ${h.console.length}, policy reports ${h.csp.length}, frame errors ${h.errors.length}`;
    },
  },
];
