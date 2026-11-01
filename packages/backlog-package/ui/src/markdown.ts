/**
 * Ticket body Markdown, copied from the Conversations Package (Packages share no code) and widened
 * for Tickets: tables and read-only task-list checkboxes. Raw HTML is never parsed, media never
 * loads, and DOMPurify sanitizes the output with a narrow allowlist.
 */
import DOMPurify from "dompurify";
import MarkdownIt from "markdown-it";
import type { RenderRule } from "markdown-it/lib/renderer.mjs";
import Token from "markdown-it/lib/token.mjs";

const SAFE_SCHEMES = /^(?:https?:|mailto:|tel:|#|\/|\.\.?\/)/i;
const SAFE_URI = /^(?:(?:https?|mailto|tel):|[^a-z][^:]*$|#|\/|\.\.?\/)/i;
const ALLOWED_TAGS = [
  "a", "blockquote", "br", "code", "del", "em", "h1", "h2", "h3", "h4", "h5", "h6", "hr", "li", "ol", "p", "pre",
  "s", "strong", "ul", "table", "thead", "tbody", "tr", "th", "td", "input",
];
const ALLOWED_ATTR = ["href", "target", "rel", "type", "checked", "disabled", "align"];
/** Attributes whose values are words, not URLs: `ALLOWED_URI_REGEXP` must not judge them. */
const WORD_ATTR = ["target", "rel", "type", "checked", "disabled", "align"];
/** A task item starts with `[ ]`, `[x]` or `[X]` followed by a space or the end of the line. */
const TASK_MARKER = /^\[([ xX])\](?:[ \t]+|$)/;

function safeLink(url: string): boolean {
  try {
    return SAFE_SCHEMES.test(decodeURIComponent(url).trim()) && SAFE_URI.test(url.trim());
  } catch {
    return false;
  }
}

function escapeText(value: string): string {
  return value.replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;").replaceAll('"', "&quot;");
}

// The default preset already parses tables and strikethrough; `html: false` escapes raw HTML.
const markdown = new MarkdownIt({ html: false, breaks: true, linkify: false, typographer: false });
markdown.validateLink = safeLink;
markdown.renderer.rules.image = (tokens: Token[], index: number): string => {
  const token = tokens[index];
  const alt = token?.children?.map((child) => child.content).join("") || token?.content || token?.attrGet("alt") || "";
  return escapeText(alt);
};
const linkRule = markdown.renderer.rules.link_open;
markdown.renderer.rules.link_open = (tokens, index, options, env, self): string => {
  const token = tokens[index];
  if (!token) return "";
  const href = token.attrGet("href") ?? "#";
  if (!safeLink(href)) token.attrSet("href", "#");
  token.attrSet("target", "_blank");
  token.attrSet("rel", "noopener noreferrer");
  return linkRule ? linkRule(tokens, index, options, env, self) : self.renderToken(tokens, index, options);
};

// markdown-it aligns table cells with an inline `style`; the sanitizer forbids `style`, so use `align`.
const alignCell: RenderRule = (tokens, index, options, _env, self): string => {
  const token = tokens[index];
  if (!token) return "";
  const style = token.attrGet("style");
  if (style !== null) {
    token.attrs = token.attrs?.filter(([name]) => name !== "style") ?? null;
    const align = /text-align:\s*(left|center|right)/.exec(style)?.[1];
    if (align) token.attrSet("align", align);
  }
  return self.renderToken(tokens, index, options);
};
markdown.renderer.rules.th_open = alignCell;
markdown.renderer.rules.td_open = alignCell;

// Task lists: a list item whose first paragraph starts with a task marker gets a disabled checkbox.
markdown.core.ruler.after("inline", "backlog_task_list", (state): boolean => {
  const tokens = state.tokens;
  for (let index = 2; index < tokens.length; index += 1) {
    const inline = tokens[index];
    if (inline?.type !== "inline" || tokens[index - 1]?.type !== "paragraph_open"
      || tokens[index - 2]?.type !== "list_item_open") continue;
    const first = inline.children?.[0];
    if (first?.type !== "text") continue;
    const marker = TASK_MARKER.exec(first.content);
    if (!marker) continue;
    first.content = first.content.slice(marker[0].length);
    const checkbox = new state.Token("backlog_task_checkbox", "input", 0);
    checkbox.meta = { checked: marker[1] !== " " };
    inline.children?.unshift(checkbox);
  }
  return false;
});
markdown.renderer.rules.backlog_task_checkbox = (tokens, index): string =>
  `<input type="checkbox" disabled${(tokens[index]?.meta as { checked?: boolean } | undefined)?.checked ? " checked" : ""}> `;

type Purifier = ReturnType<typeof DOMPurify>;
let purifier: Purifier | null = null;
/** Inputs the hook refused during the current pass; they are removed once the pass ends. */
let refused: Element[] = [];

/**
 * A Package-own DOMPurify instance whose only `input` is a disabled checkbox. The hook only marks a
 * refused `input`: removing a node while DOMPurify walks the tree could skip its next sibling.
 */
function backlogPurifier(): Purifier {
  if (purifier !== null) return purifier;
  const instance = DOMPurify(window);
  instance.addHook("afterSanitizeAttributes", (node) => {
    if (node.nodeName !== "INPUT") return;
    const element = node as Element;
    if (element.getAttribute("type")?.toLowerCase() !== "checkbox") {
      refused.push(element);
      return;
    }
    element.setAttribute("type", "checkbox");
    element.setAttribute("disabled", "");
  });
  purifier = instance;
  return instance;
}

/** Sanitizes rendered HTML to the Ticket body allowlist. Outside a browser it returns the input. */
export function sanitizeHtml(html: string): string {
  if (typeof window === "undefined") return html;
  refused = [];
  const fragment = backlogPurifier().sanitize(html, {
    ALLOWED_TAGS,
    ALLOWED_ATTR,
    FORBID_TAGS: ["img", "audio", "video", "iframe", "object", "embed", "style", "script"],
    FORBID_ATTR: ["style", "src", "srcset", "onerror", "onclick"],
    ALLOW_DATA_ATTR: false,
    ALLOWED_URI_REGEXP: SAFE_URI,
    ADD_URI_SAFE_ATTR: WORD_ATTR,
    RETURN_DOM_FRAGMENT: true,
  });
  for (const element of refused) element.remove();
  refused = [];
  const container = document.createElement("div");
  container.append(fragment);
  return container.innerHTML;
}

/** Renders a Ticket body: a small Markdown subset with tables and read-only task lists. */
export function renderMarkdown(source: string): string {
  const normalized = source.replace(/\]\(([^)]*)\)/g, (whole, destination: string) => {
    const url = destination.trim().split(/\s+/, 1)[0] ?? "";
    return safeLink(url) ? whole : "]()";
  });
  return sanitizeHtml(markdown.render(normalized));
}
