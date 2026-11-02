import { DOMParser } from "prosemirror-model";
import type { Node } from "prosemirror-model";
import { Plugin } from "prosemirror-state";
import type { NodeView } from "prosemirror-view";
import { PRESERVED_LABELS, preservedLabel, schema } from "./schema";

/** What a frame tells a Member about its source. */
export const PRESERVED_TOOLTIP = "Kept exactly as written. Edit it in a mapped file or ask an agent.";

/** Input aimed at the node itself. Pointer and drag events still reach the editor. */
const INPUT_EVENTS: ReadonlySet<string> = new Set([
  "beforeinput", "input", "keydown", "keypress", "keyup", "compositionstart", "compositionupdate", "compositionend", "paste", "cut",
]);

/**
 * The read-only frame of one `preserved_block`: a label above the source in monospace. It has no
 * `contentDOM`, so ProseMirror treats it as one leaf that is selected, moved and deleted whole.
 */
export class PreservedBlockView implements NodeView {
  readonly dom: HTMLElement;
  private readonly source: HTMLElement;
  private readonly kind: unknown;

  constructor(node: Node) {
    this.kind = node.attrs.kind;
    const label = preservedLabel(this.kind);
    this.dom = document.createElement("figure");
    this.dom.className = "preserved-block";
    this.dom.dataset.kind = String(this.kind);
    this.dom.setAttribute("role", "figure");
    this.dom.setAttribute("aria-label", `${label}. ${PRESERVED_TOOLTIP}`);
    this.dom.title = PRESERVED_TOOLTIP;
    const caption = document.createElement("figcaption");
    caption.textContent = label;
    this.source = document.createElement("pre");
    this.source.textContent = node.textContent;
    this.dom.append(caption, this.source);
  }

  update(node: Node): boolean {
    if (node.type !== schema.nodes.preserved_block || node.attrs.kind !== this.kind) return false;
    if (this.source.textContent !== node.textContent) this.source.textContent = node.textContent;
    return true;
  }

  stopEvent(event: Event): boolean {
    return INPUT_EVENTS.has(event.type);
  }

  ignoreMutation(): boolean {
    return true;
  }
}

/**
 * The clipboard parser: a frame this editor copied comes back as the same Preserved Block, while
 * everything else parses by the schema. A frame of an unknown kind is not one.
 */
const clipboardParser = new DOMParser(schema, [
  {
    tag: "figure.preserved-block",
    node: "preserved_block",
    contentElement: "pre",
    preserveWhitespace: "full",
    getAttrs: (dom) => {
      const kind = dom.getAttribute("data-kind");
      return kind !== null && Object.hasOwn(PRESERVED_LABELS, kind) ? { kind } : false;
    },
  },
  ...DOMParser.fromSchema(schema).rules,
]);

/** Shows every `preserved_block` in its frame, and keeps it whole through copy and paste. */
export function preservedPlugin(): Plugin {
  return new Plugin({
    props: {
      nodeViews: { preserved_block: (node: Node) => new PreservedBlockView(node) },
      clipboardParser,
    },
  });
}
