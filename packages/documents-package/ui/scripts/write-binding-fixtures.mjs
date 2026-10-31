// Regenerates `packages/documents-package/fixtures/body-trees/from-binding/`: Loro snapshots that
// `loro-prosemirror` writes for the shared schema, which the Rust core must read and export
// (`backend/tests/cross_language.rs`).
//
// Each fixture starts from the Rust-written genesis (`from-rust/00-genesis.loro`), as every
// editor session does, and is edited through the binding the way its sync plugin edits: build
// the ProseMirror document from the Loro tree, then write the target document back with
// `updateLoroToPmState`. One fixture merges two concurrent editors.
//
// Run it with Node 24 (it imports the TypeScript schema directly), after regenerating
// `from-rust/`, then record the Rust exports:
//
//   BODY_TREES_BLESS=1 cargo test -p teamarness-documents-package --test cross_language
//   node packages/documents-package/ui/scripts/write-binding-fixtures.mjs
//   BODY_TREES_BLESS=1 cargo test -p teamarness-documents-package --test cross_language
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { LoroDoc } from "loro-crdt";
import { createNodeFromLoroObj, updateLoroToPmState } from "loro-prosemirror";
import { Node } from "prosemirror-model";
import { MARK_EXPAND, schema } from "../src/editor/schema.ts";

const here = path.dirname(fileURLToPath(import.meta.url));
const trees = path.resolve(here, "../../fixtures/body-trees");
const genesis = fs.readFileSync(path.join(trees, "from-rust/00-genesis.loro"));
const out = path.join(trees, "from-binding");

const text = (value, ...marks) => ({ type: "text", text: value, ...(marks.length ? { marks } : {}) });
const mark = (type, attrs) => (attrs ? { type, attrs } : { type });
const p = (...content) => ({ type: "paragraph", ...(content.length ? { content } : {}) });
const h = (level, ...content) => ({ type: "heading", attrs: { level }, content });
const li = (...content) => ({ type: "list_item", content });
const ul = (...items) => ({ type: "bullet_list", content: items });
const ol = (order, ...items) => ({ type: "ordered_list", attrs: { order }, content: items });
const quote = (...content) => ({ type: "blockquote", content });
const code = (params, value) => ({ type: "code_block", attrs: { params }, ...(value ? { content: [text(value)] } : {}) });
const doc = (...content) => ({ type: "doc", content });
const link = (href, title = null) => mark("link", { href, title });

/** One body per fixture, written by the binding over the genesis. */
const FIXTURES = {
  "typed-paragraph": doc(p(text("Hello from the editor"))),
  "all-nodes": doc(
    ...[1, 2, 3, 4, 5, 6].map((level) => h(level, text(`Heading ${level}`))),
    p(
      text("Plain "), text("strong", mark("strong")), text(" "), text("em", mark("em")), text(" "),
      text("strike", mark("strikethrough")), text(" "), text("code", mark("code")), text(" "),
      text("titled", link("https://example.com/a", "Title")), text(" "),
      text("bare", link("https://example.com/b")), text(" and a break"),
      { type: "hard_break" }, text("after it."),
    ),
    quote(p(text("A quote")), ul(li(p(text("inside a list"))))),
    ol(3, li(p(text("three")), ul(li(p(text("nested"))))), li(p(text("four")))),
    { type: "horizontal_rule" },
    code("rust", "fn main() {}"),
    code("", "plain code"),
  ),
  "overlapping-marks": doc(p(
    text("bold "), text("both", mark("em"), mark("strong")), text(" italic", mark("em")),
    text(" and "), text("linked code", mark("code"), link("https://example.com/c")),
  )),
  "nested-lists": doc(ul(
    li(p(text("one")), ol(1, li(p(text("one.a")), ul(li(p(text("deep"))))), li(p(text("one.b"))))),
    li(p(text("two"))),
  )),
  "list-item-blocks": doc(ul(li(p(text("lead")), p(text("second paragraph")), code("", "code in item")))),
  "empty-paragraphs": doc(p(), p(text("between")), p(), p()),
  "unicode": doc(p(text("emoji 😀🎉, CJK 文档编辑, combining é, RTL שלום"))),
  "markdown-characters": doc(
    p(text("# not a heading, * not a bullet, [not](a link), `not code`, \\ backslash, 1. not a list")),
    h(2, text("**literal** stars")),
  ),
  "adjacent-links": doc(p(
    text("one", link("https://example.com/1")), text("two", link("https://example.com/2")),
    text(" ", mark("strong")), text("x", mark("strong"), mark("code")),
  )),
  "nested-quotes": doc(quote(p(text("outer")), quote(p(text("inner")), quote(p(text("innermost")))))),
};

/** Writes `target` into `loro` exactly as the sync plugin does after the editor changed. */
function edit(loro, target) {
  const mapping = new Map();
  createNodeFromLoroObj(schema, loro.getMap("doc"), mapping);
  const node = Node.fromJSON(schema, target);
  node.check();
  updateLoroToPmState(loro, mapping, { doc: node });
}

function session(peer) {
  const loro = new LoroDoc();
  loro.import(genesis);
  loro.setPeerId(BigInt(peer));
  loro.configTextStyle(MARK_EXPAND);
  return loro;
}

function write(name, loro) {
  const node = createNodeFromLoroObj(schema, loro.getMap("doc"), new Map());
  node.check();
  fs.writeFileSync(path.join(out, `${name}.loro`), loro.export({ mode: "snapshot" }));
  fs.writeFileSync(path.join(out, `${name}.pm.json`), `${JSON.stringify(node.toJSON(), null, 1)}\n`);
}

fs.rmSync(out, { recursive: true, force: true });
fs.mkdirSync(out, { recursive: true });
let peer = 100;
for (const [name, target] of Object.entries(FIXTURES)) {
  const loro = session(peer++);
  edit(loro, target);
  write(name, loro);
}

// Two editors type into the same empty paragraph at once, then exchange their updates.
const alice = session(peer++);
const bob = session(peer);
const before = alice.oplogVersion();
edit(alice, doc(p(text("Alice wrote this"))));
edit(bob, doc(p(text("Bob wrote that")), p(text("and more", mark("strong")))));
alice.import(bob.export({ mode: "update", from: before }));
bob.import(alice.export({ mode: "update", from: before }));
if (JSON.stringify(alice.toJSON()) !== JSON.stringify(bob.toJSON())) throw new Error("the editors did not converge");
write("concurrent-same-paragraph", alice);

console.log(`Wrote ${Object.keys(FIXTURES).length + 1} binding fixtures to ${path.relative(process.cwd(), out)}`);
