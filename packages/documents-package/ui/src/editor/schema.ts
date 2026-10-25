import { Schema } from "prosemirror-model";

/**
 * The one body schema that the UI and the Backend share.
 *
 * `loro-prosemirror` stores a Document as `{nodeName, attributes, children}` maps of exactly these
 * node and mark names, and the Rust Markdown core (`backend/src/markdown/schema.rs`) reads and
 * writes the same tree. A change here is a change to the body format: the committed fixtures in
 * `fixtures/body-trees/` pin both sides to it.
 *
 * Lists store no spacing (`tight`): Canonical Markdown writes them tight whenever that is safe.
 */
export const schema = new Schema({
  nodes: {
    doc: { content: "block+" },
    paragraph: { content: "inline*", group: "block", parseDOM: [{ tag: "p" }], toDOM: () => ["p", 0] },
    blockquote: { content: "block+", group: "block", parseDOM: [{ tag: "blockquote" }], toDOM: () => ["blockquote", 0] },
    horizontal_rule: { group: "block", parseDOM: [{ tag: "hr" }], toDOM: () => ["div", ["hr"]] },
    heading: {
      attrs: { level: { default: 1 } },
      content: "text*",
      group: "block",
      defining: true,
      parseDOM: [1, 2, 3, 4, 5, 6].map((level) => ({ tag: `h${level}`, attrs: { level } })),
      toDOM: (node) => [`h${String(node.attrs.level)}`, 0],
    },
    code_block: {
      content: "text*",
      group: "block",
      code: true,
      defining: true,
      marks: "",
      attrs: { params: { default: "" } },
      parseDOM: [{ tag: "pre", preserveWhitespace: "full" }],
      toDOM: () => ["pre", ["code", 0]],
    },
    ordered_list: {
      content: "list_item+",
      group: "block",
      attrs: { order: { default: 1 } },
      parseDOM: [{ tag: "ol", getAttrs: (dom) => ({ order: Number((dom as HTMLElement).getAttribute("start") ?? 1) || 1 }) }],
      toDOM: (node) => ["ol", { start: node.attrs.order === 1 ? null : node.attrs.order }, 0],
    },
    bullet_list: { content: "list_item+", group: "block", parseDOM: [{ tag: "ul" }], toDOM: () => ["ul", 0] },
    list_item: { content: "paragraph block*", defining: true, parseDOM: [{ tag: "li" }], toDOM: () => ["li", 0] },
    text: { group: "inline" },
    hard_break: { inline: true, group: "inline", selectable: false, parseDOM: [{ tag: "br" }], toDOM: () => ["br"] },
  },
  marks: {
    em: { parseDOM: [{ tag: "i" }, { tag: "em" }], toDOM: () => ["em"] },
    strong: { parseDOM: [{ tag: "strong" }, { tag: "b" }], toDOM: () => ["strong"] },
    strikethrough: { parseDOM: [{ tag: "s" }, { tag: "del" }], toDOM: () => ["s"] },
    link: {
      attrs: { href: {}, title: { default: null } },
      inclusive: false,
      parseDOM: [{
        tag: "a[href]",
        getAttrs: (dom) => ({ href: (dom as HTMLElement).getAttribute("href"), title: (dom as HTMLElement).getAttribute("title") }),
      }],
      toDOM: (mark) => ["a", { href: mark.attrs.href, title: mark.attrs.title, rel: "noopener noreferrer" }, 0],
    },
    code: { code: true, parseDOM: [{ tag: "code" }], toDOM: () => ["code", 0] },
  },
});

export type MarkExpand = Readonly<{ expand: "after" | "none" }>;

/**
 * The Loro text style of every mark: a mark grows when text is typed at its end (`after`),
 * except a link, which never does (`none`).
 *
 * `loro-prosemirror` derives the same configuration from the schema (`inclusive: false` gives
 * `none`), and the Rust core configures it identically before it writes a tree.
 */
export const MARK_EXPAND: Readonly<Record<string, MarkExpand>> = Object.freeze({
  em: { expand: "after" },
  strong: { expand: "after" },
  strikethrough: { expand: "after" },
  link: { expand: "none" },
  code: { expand: "after" },
});
