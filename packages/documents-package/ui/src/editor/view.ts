import { baseKeymap, chainCommands, exitCode, lift, setBlockType, toggleMark, wrapIn } from "prosemirror-commands";
import { dropCursor } from "prosemirror-dropcursor";
import { gapCursor } from "prosemirror-gapcursor";
import { InputRule, inputRules, textblockTypeInputRule, undoInputRule, wrappingInputRule } from "prosemirror-inputrules";
import { keymap } from "prosemirror-keymap";
import { Fragment } from "prosemirror-model";
import type { MarkType, Node, NodeType } from "prosemirror-model";
import { liftListItem, sinkListItem, splitListItem, wrapInList } from "prosemirror-schema-list";
import { EditorState, Selection, TextSelection } from "prosemirror-state";
import type { Command, Plugin, Transaction } from "prosemirror-state";
import { EditorView } from "prosemirror-view";
import { LoroSyncPlugin, LoroUndoPlugin, createNodeFromLoroObj } from "loro-prosemirror";
import type { LoroDocType } from "loro-prosemirror";
import type { LoroDoc } from "loro-crdt";
import { guardPlugin } from "./guard";
import type { GuardHost, GuardRefusal } from "./guard";
import { schema } from "./schema";
import { UndoHistory, guardedRedo, guardedUndo, historyPlugin } from "./undo";

/** A Comment quote is at most this many UTF-8 bytes (the Backend's `MAX_QUOTE`). */
export const MAX_QUOTE = 2 * 1024;
/** The link prompt accepts an `href` of at most this many characters. */
export const MAX_HREF = 2048;

const nodes = schema.nodes;
const marks = schema.marks;

/** The plain text of the selection, trimmed and cut to {@link MAX_QUOTE} bytes, or `null`. */
export function selectionQuote(state: EditorState): string | null {
  const { from, to, empty } = state.selection;
  if (empty) return null;
  return boundedQuote(state.doc.textBetween(from, to, "\n", "\n"));
}

/** `text` trimmed and cut, on a character boundary, to at most {@link MAX_QUOTE} UTF-8 bytes. */
export function boundedQuote(text: string): string | null {
  const encoder = new TextEncoder();
  let quote = "";
  let bytes = 0;
  for (const character of text.trim()) {
    const size = encoder.encode(character).length;
    if (bytes + size > MAX_QUOTE) break;
    quote += character;
    bytes += size;
  }
  return quote.trim() || null;
}

/** Whether `href` is a link the editor may write: web, mail, or a relative or fragment link. */
export function acceptableHref(href: string): boolean {
  const value = href.trim();
  if (value.length === 0 || value.length > MAX_HREF || /[\s<>"]/.test(value)) return false;
  const scheme = /^([a-z][a-z0-9+.-]*):/i.exec(value);
  return scheme === null ? true : ["http", "https", "mailto"].includes(scheme[1]!.toLowerCase());
}

function isIn(state: EditorState, type: NodeType): boolean {
  const { $from } = state.selection;
  for (let depth = $from.depth; depth > 0; depth -= 1) if ($from.node(depth).type === type) return true;
  return false;
}

/** The innermost list around the selection, or `null`. */
function enclosingList(state: EditorState): NodeType | null {
  const { $from } = state.selection;
  for (let depth = $from.depth; depth > 0; depth -= 1) {
    const type = $from.node(depth).type;
    if (type === nodes.bullet_list || type === nodes.ordered_list) return type;
  }
  return null;
}

/** Wraps the selection in a list of `type`, or lifts it out when it already is one. */
function toggleList(type: NodeType): Command {
  return (state, dispatch) => enclosingList(state) === type
    ? liftListItem(nodes.list_item)(state, dispatch)
    : wrapInList(type)(state, dispatch);
}

const toggleBlockquote: Command = (state, dispatch) =>
  isIn(state, nodes.blockquote) ? lift(state, dispatch) : wrapIn(nodes.blockquote)(state, dispatch);

const insertHardBreak: Command = (state, dispatch) => {
  const { $from, $to } = state.selection;
  if (!$from.parent.inlineContent || $from.parent.type.spec.code) return false;
  if ($from.sameParent($to) && !$from.parent.canReplaceWith($from.index(), $to.index(), nodes.hard_break)) return false;
  dispatch?.(state.tr.replaceSelectionWith(nodes.hard_break.create()).scrollIntoView());
  return true;
};

const insertRule: Command = (state, dispatch) => {
  const { $from } = state.selection;
  for (let depth = $from.depth; depth >= 0; depth -= 1) {
    const index = $from.index(depth);
    if ($from.node(depth).canReplaceWith(index, index, nodes.horizontal_rule)) {
      if (dispatch) {
        const tr = state.tr.replaceSelectionWith(nodes.horizontal_rule.create());
        // Continue typing after the rule, never over it: into the next text block, or a new paragraph.
        if (!(tr.selection instanceof TextSelection)) {
          const after = tr.selection.to;
          if (tr.doc.resolve(after).nodeAfter?.isTextblock !== true) tr.insert(after, nodes.paragraph.create());
          tr.setSelection(TextSelection.create(tr.doc, after + 1));
        }
        dispatch(tr.scrollIntoView());
      }
      return true;
    }
  }
  return false;
};

/** A paragraph holding only a code fence (three backticks and an optional language) becomes a code block on Enter. */
const fenceOnEnter: Command = (state, dispatch) => {
  const { $from, empty } = state.selection;
  if (!empty || $from.parent.type !== nodes.paragraph || $from.parentOffset !== $from.parent.content.size) return false;
  const match = /^```([A-Za-z0-9_+-]*)$/.exec($from.parent.textContent);
  if (match === null) return false;
  if (dispatch) {
    const start = $from.start();
    const tr = state.tr.delete(start, $from.end());
    tr.setBlockType(start, start, nodes.code_block, { params: match[1] ?? "" });
    dispatch(tr.scrollIntoView());
  }
  return true;
};

/** The range of the link mark around an empty selection, or the selection itself. */
function linkTarget(state: EditorState): { from: number; to: number } | null {
  const { from, to, empty, $from } = state.selection;
  if (!empty) return { from, to };
  const parent = $from.parent;
  const start = $from.start();
  const offset = $from.parentOffset;
  let result: { from: number; to: number } | null = null;
  let run: { from: number; to: number; href: unknown } | null = null;
  let position = 0;
  for (let index = 0; index < parent.childCount; index += 1) {
    const child = parent.child(index);
    const link = marks.link.isInSet(child.marks);
    const end = position + child.nodeSize;
    if (link && run !== null && run.href === link.attrs.href && run.to === position) run.to = end;
    else run = link ? { from: position, to: end, href: link.attrs.href } : null;
    if (run !== null && run.from <= offset && offset <= run.to) result = { from: start + run.from, to: start + run.to };
    position = end;
  }
  return result;
}

/** Sets the link of the selection (or of the link around the cursor) to `href`; `null` removes it. */
export function setLink(href: string | null): Command {
  return (state, dispatch) => {
    const target = linkTarget(state);
    if (target === null || target.from === target.to) return false;
    if (href !== null && !acceptableHref(href)) return false;
    if (dispatch) {
      const tr = state.tr.removeMark(target.from, target.to, marks.link);
      if (href !== null) tr.addMark(target.from, target.to, marks.link.create({ href: href.trim(), title: null }));
      dispatch(tr.scrollIntoView());
    }
    return true;
  };
}

/** The current link `href` around the selection, if any. */
export function currentHref(state: EditorState): string | null {
  const { $from, from, to } = state.selection;
  let href: string | null = null;
  const check = (node: Node) => {
    const link = marks.link.isInSet(node.marks);
    if (link && href === null) href = String(link.attrs.href);
  };
  if (from === to) {
    const after = $from.nodeAfter;
    const before = $from.nodeBefore;
    if (after) check(after);
    if (before) check(before);
  } else state.doc.nodesBetween(from, to, (node) => { if (node.isText) check(node); });
  return href;
}

/** Every baseline block and mark, as the toolbar and the keymap invoke them. */
export const editorCommands = {
  paragraph: setBlockType(nodes.paragraph),
  heading: (level: number): Command => setBlockType(nodes.heading, { level }),
  codeBlock: setBlockType(nodes.code_block, { params: "" }),
  bulletList: toggleList(nodes.bullet_list),
  orderedList: toggleList(nodes.ordered_list),
  blockquote: toggleBlockquote,
  horizontalRule: insertRule,
  hardBreak: insertHardBreak,
  strong: toggleMark(marks.strong),
  em: toggleMark(marks.em),
  strikethrough: toggleMark(marks.strikethrough),
  code: toggleMark(marks.code),
  undo: guardedUndo,
  redo: guardedRedo,
} as const;

/**
 * A rule that turns `**text**`-style delimiters into a mark as the closing delimiter is typed.
 * The delimiters are removed and the mark is not carried into the next character.
 */
function markRule(pattern: RegExp, type: MarkType): InputRule {
  return new InputRule(pattern, (state, match, start, end) => {
    const full = match[0];
    const inner = match[1] ?? "";
    const delimiter = (full.length - inner.length) / 2;
    const $start = state.doc.resolve(start);
    if (!$start.parent.type.allowsMarkType(type) || $start.parent.type.spec.code) return null;
    const innerStart = start + delimiter;
    const innerEnd = innerStart + inner.length;
    if (innerEnd > end) return null;
    const tr = state.tr.delete(innerEnd, end).delete(start, innerStart);
    tr.addMark(start, start + inner.length, type.create());
    tr.removeStoredMark(type);
    return tr;
  });
}

/** `---`, `***` or `___` alone in a paragraph becomes a horizontal rule. */
const ruleInputRule = new InputRule(/^(?:---|\*\*\*|___)$/, (state, _match, start, end) => {
  const $start = state.doc.resolve(start);
  if ($start.parent.type !== nodes.paragraph || $start.parent.content.size !== end - $start.start()) return null;
  const parentDepth = $start.depth - 1;
  const container = $start.node(parentDepth);
  const index = $start.index(parentDepth);
  if (!container.canReplace(index, index + 1, Fragment.from([nodes.horizontal_rule.create(), nodes.paragraph.create()]))) return null;
  const tr = state.tr.replaceWith($start.before(), $start.after(), [nodes.horizontal_rule.create(), nodes.paragraph.create()]);
  return tr.setSelection(TextSelection.create(tr.doc, $start.before() + 2));
});

/** Markdown-style shortcuts while typing. They create rendered structure; no Markdown is kept. */
export function editorInputRules(): Plugin {
  return inputRules({
    rules: [
      textblockTypeInputRule(/^(#{1,6})\s$/, nodes.heading, (match) => ({ level: match[1]!.length })),
      wrappingInputRule(/^\s*>\s$/, nodes.blockquote),
      wrappingInputRule(/^(\d+)\.\s$/, nodes.ordered_list, (match) => ({ order: Number(match[1]) }),
        (match, node) => node.childCount + Number(node.attrs.order) === Number(match[1])),
      wrappingInputRule(/^\s*([-+*])\s$/, nodes.bullet_list),
      textblockTypeInputRule(/^```([A-Za-z0-9_+-]*)\s$/, nodes.code_block, (match) => ({ params: match[1] ?? "" })),
      ruleInputRule,
      markRule(/(?<![*\w])\*\*([^*\s](?:[^*]*[^*\s])?)\*\*$/, marks.strong),
      markRule(/(?<![_\w])__([^_\s](?:[^_]*[^_\s])?)__$/, marks.strong),
      markRule(/(?<![*\w])\*([^*\s](?:[^*]*[^*\s])?)\*$/, marks.em),
      markRule(/(?<![_\w])_([^_\s](?:[^_]*[^_\s])?)_$/, marks.em),
      markRule(/(?<!~)~~([^~\s](?:[^~]*[^~\s])?)~~$/, marks.strikethrough),
      markRule(/(?<!`)`([^`]+)`$/, marks.code),
    ],
  });
}

/** The editor's keyboard shortcuts; `openLink` shows the link prompt. */
export function editorKeymap(openLink: () => boolean): Record<string, Command> {
  const bindings: Record<string, Command> = {
    "Mod-z": guardedUndo,
    "Shift-Mod-z": guardedRedo,
    "Mod-y": guardedRedo,
    Backspace: undoInputRule,
    "Mod-b": editorCommands.strong,
    "Mod-i": editorCommands.em,
    "Mod-`": editorCommands.code,
    "Mod-e": editorCommands.code,
    "Shift-Mod-s": editorCommands.strikethrough,
    "Shift-Mod-x": editorCommands.strikethrough,
    "Mod-k": () => openLink(),
    "Shift-Ctrl-0": editorCommands.paragraph,
    "Shift-Ctrl-\\": editorCommands.codeBlock,
    "Shift-Ctrl-8": editorCommands.bulletList,
    "Shift-Ctrl-9": editorCommands.orderedList,
    "Ctrl->": editorCommands.blockquote,
    "Mod-_": editorCommands.horizontalRule,
    "Shift-Enter": chainCommands(exitCode, insertHardBreak),
    "Mod-Enter": chainCommands(exitCode, insertHardBreak),
    Enter: chainCommands(fenceOnEnter, splitListItem(nodes.list_item)),
    "Mod-[": liftListItem(nodes.list_item),
    "Mod-]": sinkListItem(nodes.list_item),
  };
  for (let level = 1; level <= 6; level += 1) bindings[`Shift-Ctrl-${level}`] = editorCommands.heading(level);
  return bindings;
}

/** Every plugin of a rendered editor bound to one Loro document. */
export function editorPlugins(doc: LoroDoc, guard: GuardHost, openLink: () => boolean): Plugin[] {
  const loro = doc as unknown as LoroDocType;
  // Per-person undo keeps every change that cannot be undone in one record out of its history.
  const history = new UndoHistory(doc);
  return [
    guardPlugin(guard, { batched: (apply) => history.batched(apply) }),
    LoroSyncPlugin({ doc: loro }),
    historyPlugin(history),
    LoroUndoPlugin({ doc, undoManager: history.manager }),
    editorInputRules(),
    keymap(editorKeymap(openLink)),
    keymap(baseKeymap),
    dropCursor(),
    gapCursor(),
  ];
}

/** The ProseMirror document the Loro body holds now. */
export function initialDocument(doc: LoroDoc): Node {
  return createNodeFromLoroObj(schema, (doc as unknown as LoroDocType).getMap("doc"), new Map());
}

/** Whether the Loro document holds a body the editor can show (it has its genesis). */
export function hasBody(doc: LoroDoc): boolean {
  return (doc as unknown as LoroDocType).getMap("doc").size > 0;
}

type ToolbarItem = Readonly<{
  label: string;
  title: string;
  command: Command;
  active?: (state: EditorState) => boolean;
}>;

function markActive(type: MarkType) {
  return (state: EditorState): boolean => {
    const { from, $from, to, empty } = state.selection;
    if (empty) return type.isInSet(state.storedMarks ?? $from.marks()) !== undefined;
    return state.doc.rangeHasMark(from, to, type);
  };
}

const REFUSAL_TEXT: Readonly<Record<GuardRefusal, string>> = {
  block_too_large: "That is too large to add as one block. Split it into smaller paragraphs and try again.",
  document_too_large: "This Document has reached its size limit. Remove some content before adding more.",
  step_too_large: "That change is too large to undo or redo.",
  step_unknown: "That change could not be checked, so it cannot be undone or redone.",
};

export type DocumentEditorOptions = Readonly<{
  guard: Omit<GuardHost, "refused">;
  onCopyMarkdown: () => void;
  onComment: () => void;
}>;

/**
 * The rendered editor of one open Document: a toolbar and a ProseMirror view bound to the
 * session's Loro document through `loro-prosemirror`. It never shows Markdown source.
 */
export class DocumentEditor {
  readonly element: HTMLElement;
  private readonly toolbar: HTMLElement;
  private readonly surface: HTMLElement;
  private readonly notice: HTMLElement;
  private readonly linkForm: HTMLFormElement;
  private readonly linkInput: HTMLInputElement;
  private readonly linkError: HTMLElement;
  private readonly buttons: Array<{ element: HTMLButtonElement; item: ToolbarItem }> = [];
  private readonly actionButtons: HTMLButtonElement[] = [];
  private readonly options: DocumentEditorOptions;
  private view: EditorView | null = null;
  private doc: LoroDoc | null = null;
  private editable = false;
  private linkRange: { from: number; to: number } | null = null;

  constructor(options: DocumentEditorOptions) {
    this.options = options;
    this.element = document.createElement("div");
    this.element.className = "document-editor";
    this.toolbar = document.createElement("div");
    this.toolbar.className = "editor-toolbar";
    this.toolbar.setAttribute("role", "toolbar");
    this.toolbar.setAttribute("aria-label", "Formatting");

    // Block styles are buttons, not a `<select>`: WebKit's own select rendering trips the
    // frame's `style-src` policy.
    const groups: ToolbarItem[][] = [
      [
        { label: "¶", title: "Paragraph (⇧⌃0)", command: editorCommands.paragraph, active: blockIs("paragraph") },
        ...[1, 2, 3, 4, 5, 6].map((level): ToolbarItem => ({
          label: `H${level}`, title: `Heading ${level} (⇧⌃${level})`, command: editorCommands.heading(level), active: blockIs(`heading${level}`),
        })),
        { label: "{ }", title: "Code block (⇧⌃\\)", command: editorCommands.codeBlock, active: blockIs("code_block") },
      ],
      [
        { label: "B", title: "Bold (⌘B)", command: editorCommands.strong, active: markActive(marks.strong) },
        { label: "I", title: "Italic (⌘I)", command: editorCommands.em, active: markActive(marks.em) },
        { label: "S", title: "Strikethrough (⇧⌘S)", command: editorCommands.strikethrough, active: markActive(marks.strikethrough) },
        { label: "Code", title: "Inline code (⌘E)", command: editorCommands.code, active: markActive(marks.code) },
        { label: "Link", title: "Link (⌘K)", command: (_state, dispatch) => dispatch === undefined || this.openLink(), active: markActive(marks.link) },
      ],
      [
        { label: "Bullets", title: "Bulleted list (⇧⌃8)", command: editorCommands.bulletList, active: (state) => enclosingList(state) === nodes.bullet_list },
        { label: "Numbers", title: "Numbered list (⇧⌃9)", command: editorCommands.orderedList, active: (state) => enclosingList(state) === nodes.ordered_list },
        { label: "Quote", title: "Blockquote (⌃>)", command: editorCommands.blockquote, active: (state) => isIn(state, nodes.blockquote) },
        { label: "Rule", title: "Horizontal rule (⌘_)", command: editorCommands.horizontalRule },
        { label: "Break", title: "Line break (⇧↵)", command: editorCommands.hardBreak },
      ],
      [
        { label: "Undo", title: "Undo (⌘Z)", command: guardedUndo },
        { label: "Redo", title: "Redo (⇧⌘Z)", command: guardedRedo },
      ],
    ];
    for (const group of groups) {
      const wrapper = document.createElement("span");
      wrapper.className = "editor-toolbar-group";
      for (const item of group) {
        const element = document.createElement("button");
        element.type = "button";
        element.className = "editor-tool";
        element.textContent = item.label;
        element.title = item.title;
        element.setAttribute("aria-label", item.title.replace(/ \(.*\)$/, ""));
        if (item.active) element.setAttribute("aria-pressed", "false");
        // Keep the editor's selection: a toolbar press must not move focus first.
        element.addEventListener("mousedown", (event) => event.preventDefault());
        element.addEventListener("click", () => this.run(item.command));
        wrapper.append(element);
        this.buttons.push({ element, item });
      }
      this.toolbar.append(wrapper);
    }
    const actions = document.createElement("span");
    actions.className = "editor-toolbar-group editor-toolbar-actions";
    const copy = document.createElement("button");
    copy.type = "button";
    copy.className = "editor-tool editor-copy-markdown";
    copy.textContent = "Copy as Markdown";
    copy.addEventListener("click", () => options.onCopyMarkdown());
    const comment = document.createElement("button");
    comment.type = "button";
    comment.className = "editor-tool editor-comment";
    comment.textContent = "Comment";
    comment.title = "Comment on the selected text";
    comment.addEventListener("mousedown", (event) => event.preventDefault());
    comment.addEventListener("click", () => options.onComment());
    actions.append(copy, comment);
    this.actionButtons.push(copy, comment);
    this.toolbar.append(actions);

    this.linkForm = document.createElement("form");
    this.linkForm.className = "editor-link-prompt teamarness-field";
    this.linkForm.hidden = true;
    this.linkInput = document.createElement("input");
    this.linkInput.className = "teamarness-input";
    this.linkInput.type = "text";
    this.linkInput.maxLength = MAX_HREF;
    this.linkInput.placeholder = "https://example.com";
    this.linkInput.setAttribute("aria-label", "Link address");
    this.linkError = document.createElement("span");
    this.linkError.className = "editor-link-error";
    this.linkError.setAttribute("role", "alert");
    const apply = document.createElement("button");
    apply.type = "submit";
    apply.className = "teamarness-control teamarness-control--primary";
    apply.textContent = "Apply";
    const remove = document.createElement("button");
    remove.type = "button";
    remove.className = "teamarness-control teamarness-control--secondary editor-link-remove";
    remove.textContent = "Remove link";
    remove.addEventListener("click", () => this.closeLink(null));
    const cancel = document.createElement("button");
    cancel.type = "button";
    cancel.className = "teamarness-control teamarness-control--secondary";
    cancel.textContent = "Cancel";
    cancel.addEventListener("click", () => this.closeLink(undefined));
    this.linkForm.append(this.linkInput, apply, remove, cancel, this.linkError);
    const submit = (event: Event): void => {
      event.preventDefault();
      const href = this.linkInput.value.trim();
      if (!acceptableHref(href)) {
        this.linkError.textContent = "Enter a web, mail, or relative link address.";
        return;
      }
      this.closeLink(href);
    };
    // A sandboxed frame may not submit forms, so the Apply press itself applies the link (Enter
    // in the field presses Apply); the submit event is handled for engines that still fire it.
    apply.addEventListener("click", submit);
    this.linkForm.addEventListener("submit", submit);
    this.linkForm.addEventListener("keydown", (event) => {
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        this.closeLink(undefined);
      }
    });

    this.notice = document.createElement("p");
    this.notice.className = "teamarness-notice editor-notice";
    this.notice.setAttribute("role", "alert");
    this.notice.hidden = true;

    this.surface = document.createElement("div");
    this.surface.className = "editor-surface";
    this.element.append(this.toolbar, this.linkForm, this.notice, this.surface);
    this.refreshToolbar();
  }

  /** The element that scrolls the body. */
  get scroller(): HTMLElement {
    return this.surface;
  }

  /** The ProseMirror view, while a body is shown. */
  get editorView(): EditorView | null {
    return this.view;
  }

  /**
   * Shows `doc` (or nothing). A new doc — the session reloaded after a resync — replaces the view;
   * the same doc only updates whether it can be edited.
   */
  bind(doc: LoroDoc | null, editable: boolean): void {
    this.editable = editable;
    if (doc !== this.doc) {
      this.view?.destroy();
      this.view = null;
      this.surface.replaceChildren();
      this.doc = doc;
      this.closeLink(undefined);
      if (doc !== null) this.view = this.createView(doc);
    } else {
      this.view?.setProps({});
    }
    this.refreshToolbar();
  }

  /** The selection's plain text as a Comment quote. */
  quote(): string | null {
    return this.view ? selectionQuote(this.view.state) : null;
  }

  /** Collapses the selection to its end (after a quoted Comment was posted). */
  collapseSelection(): void {
    const view = this.view;
    if (view === null || view.state.selection.empty) return;
    view.dispatch(view.state.tr.setSelection(Selection.near(view.state.doc.resolve(view.state.selection.to), -1)));
  }

  focus(): void {
    this.view?.focus();
  }

  destroy(): void {
    this.view?.destroy();
    this.view = null;
    this.doc = null;
    this.element.remove();
  }

  private createView(doc: LoroDoc): EditorView {
    const guard: GuardHost = {
      unsentBytes: () => this.options.guard.unsentBytes(),
      flush: () => this.options.guard.flush(),
      refused: (reason) => this.showNotice(REFUSAL_TEXT[reason]),
    };
    const state = EditorState.create({
      schema,
      doc: initialDocument(doc),
      plugins: editorPlugins(doc, guard, () => this.openLink()),
    });
    const view: EditorView = new EditorView(this.surface, {
      state,
      editable: () => this.editable,
      attributes: { "aria-label": "Document body", "aria-multiline": "true", role: "textbox", spellcheck: "true" },
      dispatchTransaction: (tr: Transaction) => {
        view.updateState(view.state.apply(tr));
        this.refreshToolbar();
      },
      handleDOMEvents: {
        // A link in the body never navigates the frame.
        click: (_view, event) => {
          if ((event.target as Element | null)?.closest?.("a")) event.preventDefault();
          return false;
        },
      },
    });
    return view;
  }

  private run(command: Command): void {
    const view = this.view;
    if (view === null || !this.editable) return;
    command(view.state, view.dispatch, view);
    if (this.linkForm.hidden) view.focus();
  }

  private openLink(): boolean {
    const view = this.view;
    if (view === null || !this.editable) return false;
    const target = linkTarget(view.state);
    if (target === null || target.from === target.to) {
      this.showNotice("Select the text to link first.");
      return true;
    }
    this.linkRange = target;
    this.linkInput.value = currentHref(view.state) ?? "";
    this.linkError.textContent = "";
    this.linkForm.hidden = false;
    this.linkInput.focus();
    this.linkInput.select();
    return true;
  }

  /** Closes the prompt: a string applies it, `null` removes the link, `undefined` cancels. */
  private closeLink(href: string | null | undefined): void {
    const wasOpen = !this.linkForm.hidden;
    this.linkForm.hidden = true;
    const range = this.linkRange;
    this.linkRange = null;
    const view = this.view;
    if (!wasOpen || view === null) return;
    if (href !== undefined && range !== null && this.editable) {
      const size = view.state.doc.content.size;
      const from = Math.min(range.from, size);
      const to = Math.min(range.to, size);
      if (from < to) {
        const tr = view.state.tr.removeMark(from, to, marks.link);
        if (href !== null) tr.addMark(from, to, marks.link.create({ href, title: null }));
        view.dispatch(tr.scrollIntoView());
      }
    }
    view.focus();
  }

  private showNotice(text: string): void {
    this.notice.textContent = text;
    this.notice.hidden = false;
  }

  private refreshToolbar(): void {
    const view = this.view;
    const state = view?.state ?? null;
    const usable = state !== null && this.editable;
    for (const { element, item } of this.buttons) {
      element.disabled = state === null || !usable || !item.command(state, undefined, view ?? undefined);
      if (item.active) element.setAttribute("aria-pressed", String(state !== null && item.active(state)));
    }
    for (const action of this.actionButtons) action.disabled = this.view === null;
  }
}

function blockIs(value: string) {
  return (state: EditorState): boolean => blockValue(state) === value;
}

function blockValue(state: EditorState): string {
  const parent = state.selection.$from.parent;
  if (parent.type === nodes.heading) return `heading${String(parent.attrs.level)}`;
  if (parent.type === nodes.code_block) return "code_block";
  return "paragraph";
}
