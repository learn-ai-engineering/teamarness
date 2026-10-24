import type { Collection, Comment, Document, TextEvent } from "./contract.generated";
import type { DocumentsController } from "./controller";
import { renderMarkdown } from "./markdown";

const KIT = {
  primary: "teamarness-control teamarness-control--primary",
  secondary: "teamarness-control teamarness-control--secondary",
  field: "teamarness-field",
  input: "teamarness-input",
  notice: "teamarness-notice",
  noticeDanger: "teamarness-notice teamarness-notice--danger",
} as const;

const AVAILABILITY_LABEL = {
  current: "Up to date",
  catching_up: "Catching up",
  offline: "Offline",
  waiting_for_history_access: "Waiting for history access",
} as const;

function boundedQuote(selection: string): string | null {
  const encoder = new TextEncoder();
  let quote = "";
  let bytes = 0;
  for (const character of selection.trim()) {
    const size = encoder.encode(character).length;
    if (bytes + size > 2048) break;
    quote += character;
    bytes += size;
  }
  return quote || null;
}

function button(label: string, className: string, onClick: () => void): HTMLButtonElement {
  const element = document.createElement("button");
  element.type = "button";
  element.className = className;
  element.textContent = label;
  element.addEventListener("click", onClick);
  return element;
}

const ICON_PATHS = {
  plus: "M12 5v14 M5 12h14", x: "M5 5l14 14 M19 5L5 19",
  chevron: "m9 18 6-6-6-6", more: "M5 12h.01 M12 12h.01 M19 12h.01",
  panel: "M4 4h16v16H4z M9 4v16", comments: "M4 4h16v16H4z M15 4v16",
} as const;

function icon(name: keyof typeof ICON_PATHS, label: string, onClick: () => void): HTMLButtonElement {
  const control = button("", "documents-icon", onClick);
  control.setAttribute("aria-label", label);
  control.title = label;
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("aria-hidden", "true");
  const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
  path.setAttribute("d", ICON_PATHS[name]);
  svg.append(path);
  control.append(svg);
  return control;
}

/** A one-field form built once; the view only patches its disabled state. */
function inlineForm(options: {
  label: string;
  placeholder: string;
  submitLabel: string;
  maxLength: number;
  className: string;
  onSubmit: (value: string) => Promise<boolean>;
  currentDraft?: () => number;
}): { form: HTMLFormElement; input: HTMLInputElement; submit: HTMLButtonElement } {
  const form = document.createElement("form");
  form.className = `${options.className} ${KIT.field}`;
  const input = document.createElement("input");
  input.className = KIT.input;
  input.type = "text";
  input.required = true;
  // HTML maxLength counts UTF-16 units, while the published limit counts characters.
  input.maxLength = options.maxLength * 2;
  let inputRevision = 0;
  input.addEventListener("input", () => { inputRevision++; input.setCustomValidity(""); });
  input.placeholder = options.placeholder;
  input.setAttribute("aria-label", options.label);
  const submit = document.createElement("button");
  submit.type = "submit";
  submit.className = KIT.primary;
  submit.textContent = options.submitLabel;
  form.append(input, submit);
  form.addEventListener("submit", (event) => {
    event.preventDefault();
    const value = input.value.trim();
    if (!value) return;
    if ([...value].length > options.maxLength) {
      input.setCustomValidity(`${options.label} must be at most ${options.maxLength} characters.`);
      input.reportValidity();
      return;
    }
    const revision = inputRevision;
    const draft = options.currentDraft?.();
    void options.onSubmit(value).then((ok) => {
      if (ok && revision === inputRevision && draft === options.currentDraft?.()) input.value = "";
    });
  });
  return { form, input, submit };
}

/**
 * Builds the Documents DOM once and updates regions in place. The create forms persist across
 * renders so a remote change never discards what a Member is typing.
 */
export class DocumentsView {
  private readonly page: HTMLElement;
  private readonly status: HTMLElement;
  private readonly warnings: HTMLElement;
  private readonly error: HTMLElement;
  private readonly collectionList: HTMLUListElement;
  private readonly collectionForm: ReturnType<typeof inlineForm>;
  private readonly documentForm: ReturnType<typeof inlineForm>;
  private readonly archivedToggle: HTMLInputElement;
  private readonly editorPane: HTMLElement;
  private readonly editorTitle: HTMLElement;
  private readonly editorStatus: HTMLElement;
  private readonly textarea: HTMLTextAreaElement;
  private readonly rail: HTMLElement;
  private readonly shell: HTMLElement;
  private readonly leftHandle: HTMLElement;
  private readonly rightHandle: HTMLElement;
  private readonly resizeRanges: Array<() => void> = [];
  private readonly resizeObserver: ResizeObserver | null;
  private readonly showNavigation: HTMLButtonElement;
  private readonly commentsToggle: HTMLButtonElement;
  private readonly previewButton: HTMLButtonElement;
  private readonly preview: HTMLElement;
  private readonly commentsPanel: HTMLElement;
  private readonly commentList: HTMLElement;
  private readonly commentForm: HTMLFormElement;
  private readonly commentInput: HTMLTextAreaElement;
  private readonly commentQuote: HTMLElement;
  private readonly commentSubmit: HTMLButtonElement;
  private readonly controller: DocumentsController;
  private draftQuote: string | null = null;
  private replyTo: string | null = null;
  private composerDocumentId: string | null = null;
  private readonly expanded = new Set<string>();
  private selectedCollectionId: string | null = null;
  private leftOpen = true;
  private commentsOpen = true;
  private createTarget: string | null = null;
  private createGeneration = 0;
  private activeMenu: HTMLElement | null = null;
  private activeMenuButton: HTMLButtonElement | null = null;
  private renameForm: ReturnType<typeof inlineForm> | null = null;
  private renameTarget: string | null = null;
  private readonly onOutsideClick = (event: Event): void => {
    if (this.activeMenu && !event.composedPath().includes(this.activeMenu)
      && !event.composedPath().includes(this.activeMenuButton as EventTarget)) this.dismissMenu();
  };
  private readonly onEscape = (event: Event): void => {
    if ((event as KeyboardEvent).key === "Escape" && this.activeMenu) {
      const trigger = this.activeMenuButton;
      this.dismissMenu();
      trigger?.focus();
      (event as KeyboardEvent).preventDefault();
    }
  };

  constructor(root: ShadowRoot | HTMLElement, controller: DocumentsController, title: string) {
    this.controller = controller;
    this.page = document.createElement("div");
    this.page.className = "teamarness-page documents-page";

    const header = document.createElement("header");
    header.className = "teamarness-page-header";
    const heading = document.createElement("h1");
    heading.className = "teamarness-page-title";
    heading.textContent = title;
    this.status = document.createElement("p");
    this.status.className = "availability";
    this.status.setAttribute("role", "status");
    this.showNavigation = icon("panel", "Show navigation", () => { this.leftOpen = true; this.renderRails(); });
    this.showNavigation.hidden = true;
    this.commentsToggle = icon("comments", "Hide Comments", () => {
      this.commentsOpen = !this.commentsOpen;
      this.renderRails();
    });
    this.commentsToggle.classList.add("comments-toggle");
    const headerActions = document.createElement("div");
    headerActions.className = "documents-header-actions";
    headerActions.append(this.showNavigation, this.commentsToggle);
    const archivedLabel = document.createElement("label");
    archivedLabel.className = "archived-toggle";
    this.archivedToggle = document.createElement("input");
    this.archivedToggle.type = "checkbox";
    this.archivedToggle.addEventListener("change", () => controller.toggleArchived());
    archivedLabel.append(this.archivedToggle, document.createTextNode(" Show archived"));
    header.append(heading, this.status, headerActions);

    this.warnings = document.createElement("div");
    this.warnings.className = "documents-warnings";
    this.error = document.createElement("p");
    this.error.className = KIT.noticeDanger;
    this.error.setAttribute("role", "alert");
    this.error.hidden = true;

    this.shell = document.createElement("div");
    this.shell.className = "documents-shell";

    this.rail = document.createElement("aside");
    this.rail.className = "collection-rail";
    this.rail.setAttribute("aria-label", "Collections and Documents");
    const railHeader = document.createElement("div");
    railHeader.className = "collection-rail-header";
    const railHeading = document.createElement("h2");
    railHeading.textContent = "Collections";
    const hideNavigation = icon("panel", "Hide navigation", () => { this.leftOpen = false; this.renderRails(); });
    const addCollection = icon("plus", "Add Collection", () => {
      this.createGeneration++;
      this.createTarget = "collection";
      this.render();
      this.collectionForm.input.focus();
    });
    railHeader.append(hideNavigation, railHeading, archivedLabel, addCollection);
    this.collectionList = document.createElement("ul");
    this.collectionList.className = "collection-list";
    this.collectionForm = inlineForm({
      label: "New Collection name", placeholder: "New Collection", submitLabel: "Add",
      maxLength: 128, className: "collection-create", currentDraft: () => this.createGeneration,
      onSubmit: async (name) => {
        const generation = this.createGeneration;
        const ok = await controller.createCollection(name);
        if (ok && generation === this.createGeneration && this.createTarget === "collection") {
          this.createTarget = null;
          this.render();
        }
        return ok;
      },
    });
    this.documentForm = inlineForm({
      label: "New Document title", placeholder: "New Document title", submitLabel: "Create",
      maxLength: 256, className: "document-create", currentDraft: () => this.createGeneration,
      onSubmit: async (title) => {
        const target = this.createTarget;
        const generation = this.createGeneration;
        if (target === null || target === "collection"
          || target !== controller.state.selectedCollectionId) return false;
        const ok = await controller.createDocument(title);
        if (ok && generation === this.createGeneration && this.createTarget === target) {
          this.createTarget = null;
          this.render();
        }
        return ok;
      },
    });
    this.collectionForm.input.addEventListener("input", () => { this.createGeneration++; });
    this.documentForm.input.addEventListener("input", () => { this.createGeneration++; });
    this.collectionForm.form.append(icon("x", "Cancel", () => { this.createGeneration++; this.createTarget = null; this.render(); }));
    this.documentForm.form.append(icon("x", "Cancel", () => { this.createGeneration++; this.createTarget = null; this.render(); }));
    this.rail.append(railHeader, this.collectionList);
    this.leftHandle = this.resizeHandle("left");
    this.rightHandle = this.resizeHandle("right");

    // --- editor pane (built once; shown when a Document is open) ---
    this.editorPane = document.createElement("section");
    this.editorPane.className = "editor-pane";
    this.editorPane.hidden = true;
    const editorHeader = document.createElement("div");
    editorHeader.className = "editor-header";
    this.editorTitle = document.createElement("h2");
    this.editorTitle.className = "editor-title";
    this.editorStatus = document.createElement("span");
    this.editorStatus.className = "editor-status";
    this.editorStatus.setAttribute("role", "status");
    this.previewButton = button("Preview", KIT.secondary, () => controller.togglePreview());
    this.previewButton.disabled = true;
    this.previewButton.setAttribute("aria-pressed", "false");
    editorHeader.append(this.editorTitle, this.editorStatus, this.previewButton);
    this.preview = document.createElement("article");
    this.preview.className = "editor-preview";
    this.preview.setAttribute("aria-label", "Rendered document");
    this.preview.hidden = true;
    this.textarea = document.createElement("textarea");
    this.textarea.className = `${KIT.input} editor-textarea`;
    this.textarea.setAttribute("aria-label", "Document text (Markdown)");
    this.textarea.spellcheck = false;
    this.textarea.addEventListener("input", () => {
      controller.openEditor?.input(this.textarea.value);
    });
    const clearCollapsedQuote = () => {
      if (this.draftQuote !== null && this.textarea.selectionStart === this.textarea.selectionEnd) {
        this.draftQuote = null;
        this.renderComposer();
      }
    };
    this.textarea.addEventListener("select", clearCollapsedQuote);
    this.textarea.addEventListener("keyup", clearCollapsedQuote);
    this.textarea.addEventListener("pointerup", clearCollapsedQuote);
    this.editorPane.append(editorHeader, this.textarea, this.preview);
    controller.textListener = (event: TextEvent) => this.onTextEvent(event);

    // --- comments (built once; the list is rebuilt per render, the composer persists) ---
    this.commentsPanel = document.createElement("section");
    this.commentsPanel.className = "comments-panel";
    this.commentsPanel.setAttribute("aria-label", "Comments");
    const commentsHeading = document.createElement("h3");
    commentsHeading.textContent = "Comments";
    this.commentList = document.createElement("ol");
    this.commentList.className = "comment-list";
    this.commentForm = document.createElement("form");
    this.commentForm.className = `comment-compose ${KIT.field}`;
    this.commentQuote = document.createElement("blockquote");
    this.commentQuote.className = "comment-quote comment-quote--draft";
    this.commentQuote.hidden = true;
    this.commentInput = document.createElement("textarea");
    this.commentInput.className = `${KIT.input} comment-input`;
    this.commentInput.rows = 3;
    this.commentInput.maxLength = 32768;
    this.commentInput.placeholder = "Add a comment (Markdown)";
    this.commentInput.setAttribute("aria-label", "New comment");
    this.commentInput.addEventListener("focus", () => {
      if (this.controller.state.openDocumentId === null || !this.controller.openEditor
        || this.textarea.hidden || this.textarea.disabled) return;
      this.draftQuote = this.selectedEditorQuote();
      this.renderComposer();
    });
    this.commentSubmit = document.createElement("button");
    this.commentSubmit.type = "submit";
    this.commentSubmit.className = KIT.primary;
    this.commentSubmit.textContent = "Comment";
    const composeActions = document.createElement("div");
    composeActions.className = "comment-compose-actions";
    composeActions.append(this.commentSubmit);
    this.commentForm.append(this.commentQuote, this.commentInput, composeActions);
    this.commentForm.addEventListener("submit", (event) => {
      event.preventDefault();
      const markdown = this.commentInput.value.trim();
      if (!markdown) return;
      const documentId = controller.state.openDocumentId;
      if (this.draftQuote !== null && this.selectedEditorQuote() === null) {
        this.draftQuote = null;
        this.renderComposer();
      }
      void controller.addComment(markdown, this.draftQuote, this.replyTo).then((ok) => {
        if (ok && controller.state.openDocumentId === documentId) {
          this.commentInput.value = "";
          this.collapseEditorSelection();
          this.draftQuote = null;
          this.replyTo = null;
          this.renderComposer();
        }
      });
    });
    this.commentsPanel.append(commentsHeading, this.commentList, this.commentForm);
    this.shell.append(this.rail, this.leftHandle, this.editorPane, this.rightHandle, this.commentsPanel);
    this.page.append(header, this.warnings, this.error, this.shell);
    root.addEventListener("click", this.onOutsideClick);
    root.addEventListener("keydown", this.onEscape);
    document.addEventListener("click", this.onOutsideClick);
    root.append(this.page);
    this.resizeObserver = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(() => {
      this.resizeRanges.forEach((update) => update());
    });
    this.resizeObserver?.observe(this.shell);
  }

  /** Routes a text event through the controller with the live caret, then syncs the textarea. */
  private onTextEvent(event: TextEvent): void {
    const focused = this.textarea.ownerDocument.activeElement === this.textarea
      || (this.textarea.getRootNode() as ShadowRoot).activeElement === this.textarea;
    const caret = focused ? { start: this.textarea.selectionStart, end: this.textarea.selectionEnd } : null;
    const next = this.controller.receiveText(event, caret);
    this.syncTextarea(next);
  }

  private syncTextarea(caret: { start: number; end: number } | null): void {
    const editor = this.controller.openEditor;
    if (!editor) return;
    const visible = editor.current.reason === "import_failure" ? editor.current.text : editor.value;
    if (this.textarea.value !== visible) {
      this.textarea.value = visible;
      if (caret) this.textarea.setSelectionRange(caret.start, caret.end);
    }
  }

  private selectedEditorQuote(): string | null {
    return boundedQuote(this.textarea.value.slice(this.textarea.selectionStart, this.textarea.selectionEnd));
  }

  private collapseEditorSelection(): void {
    this.textarea.setSelectionRange(this.textarea.selectionEnd, this.textarea.selectionEnd);
  }

  private renderComposer(): void {
    const root = this.commentQuote.getRootNode() as ShadowRoot;
    const focusedCue = this.commentQuote.contains(root.activeElement)
      ? root.activeElement?.getAttribute("aria-label") : null;
    this.commentQuote.hidden = this.draftQuote === null && this.replyTo === null;
    this.commentQuote.replaceChildren();
    if (this.replyTo !== null) {
      const reply = document.createElement("span");
      reply.className = "comment-reply-to";
      reply.textContent = "Replying to a comment";
      this.commentQuote.append(reply, icon("x", "Clear reply", () => {
        this.replyTo = null;
        this.renderComposer();
      }));
    }
    if (this.draftQuote !== null) {
      const cue = document.createElement("span");
      cue.className = "comment-quote-cue";
      cue.textContent = "Commenting on highlighted text";
      const quote = document.createElement("span");
      quote.className = "comment-quote-excerpt";
      quote.textContent = this.draftQuote;
      this.commentQuote.append(cue, quote, icon("x", "Clear quote", () => {
        this.draftQuote = null;
        this.collapseEditorSelection();
        this.renderComposer();
      }));
    }
    if (focusedCue) this.commentQuote.querySelector<HTMLButtonElement>(`[aria-label="${focusedCue}"]`)?.focus();
    this.commentSubmit.disabled = this.controller.state.busy;
  }

  private commentItem(comment: Comment, depth: number): HTMLLIElement {
    const item = document.createElement("li");
    item.className = "comment";
    item.style.setProperty("--comment-depth", String(Math.min(depth, 6)));
    if (comment.resolved) item.classList.add("is-resolved");
    const meta = document.createElement("div");
    meta.className = "comment-meta";
    const author = document.createElement("span");
    author.className = "comment-author";
    author.textContent = this.controller.labelFor(comment.author, comment.isLocal);
    author.title = comment.author;
    const state = document.createElement("span");
    state.className = "comment-state";
    state.textContent = comment.resolved ? "Resolved" : "";
    const time = document.createElement("time");
    time.className = "comment-time";
    if (comment.createdAt === undefined) time.textContent = "Time unavailable";
    else {
      const date = new Date(Number(comment.createdAt));
      time.dateTime = date.toISOString();
      time.textContent = date.toLocaleString();
    }
    meta.append(author, time, state);
    item.append(meta);
    if (comment.quote !== undefined) {
      const quote = document.createElement("blockquote");
      quote.className = "comment-quote";
      quote.textContent = comment.quote;
      item.append(quote);
    }
    const body = document.createElement("div");
    body.className = "comment-body";
    if (comment.markdown !== undefined) body.innerHTML = renderMarkdown(comment.markdown);
    else {
      body.classList.add("comment-body--pending");
      body.textContent = "Waiting for history access";
    }
    item.append(body);
    const actions = document.createElement("div");
    actions.className = "comment-actions";
    actions.append(
      button("Reply", KIT.secondary, () => { this.replyTo = comment.id; this.renderComposer(); this.commentInput.focus(); }),
      comment.resolved
        ? button("Reopen", KIT.secondary, () => { void this.controller.reopenComment(comment.id); })
        : button("Resolve", KIT.secondary, () => { void this.controller.resolveComment(comment.id); }),
    );
    for (const control of actions.querySelectorAll("button")) control.disabled = this.controller.state.busy;
    item.append(actions);
    return item;
  }

  private renderComments(): void {
    const { comments, commentsCursor, commentsLoading } = this.controller.state;
    const sorted = [...comments].sort((a, b) => a.position.localeCompare(b.position) || a.id.localeCompare(b.id));
    const children = new Map<string | null, Comment[]>();
    for (const comment of sorted) {
      const key = comment.parentId ?? null;
      const siblings = children.get(key) ?? [];
      siblings.push(comment);
      children.set(key, siblings);
    }
    const items: HTMLLIElement[] = [];
    const placed = new Set<string>();
    const walk = (comment: Comment, depth: number) => {
      if (placed.has(comment.id)) return;
      placed.add(comment.id);
      items.push(this.commentItem(comment, depth));
      for (const child of children.get(comment.id) ?? []) walk(child, depth + 1);
    };
    for (const comment of sorted) {
      if (comment.parentId === undefined || !sorted.some((parent) => parent.id === comment.parentId)) walk(comment, 0);
    }
    for (const comment of sorted) walk(comment, 0);
    this.commentList.replaceChildren(...items);
    if (commentsCursor !== null) {
      const more = document.createElement("li");
      const load = button(commentsLoading ? "Loading…" : "Load older comments", "comments-load-older " + KIT.secondary,
        () => { void this.controller.loadOlderComments(); });
      load.disabled = commentsLoading;
      more.append(load);
      this.commentList.prepend(more);
    }
    this.renderComposer();
  }

  private renderEditor(): void {
    const open = this.controller.openDocument;
    const editor = this.controller.openEditor;
    this.editorPane.hidden = false;
    this.previewButton.disabled = open === null;
    this.commentsPanel.hidden = open === null || !this.commentsOpen;
    this.rightHandle.hidden = this.commentsPanel.hidden;
    if (open === null) { this.editorTitle.textContent = "Choose a Document"; this.textarea.hidden = true; this.preview.hidden = true; return; }
    this.renderComments();
    this.editorTitle.textContent = open.title;
    if (!editor) {
      this.editorStatus.textContent = "Opening…";
      this.textarea.disabled = true;
      return;
    }
    const { editable, sending, error, reason } = editor.current;
    this.textarea.disabled = !editable;
    this.editorStatus.textContent = reason === "identity_conflict"
      ? "Conflicting edit history — document unavailable. Unsaved draft remains only in this window."
      : reason === "import_failure"
        ? `Incomplete edit history — read only. Last known-good text; record ${editor.current.diagnosticRecordId ?? "unknown"}. Unsaved draft remains only in this window.`
        : error ?? (!editable ? "Waiting for history" : sending || editor.value !== editor.current.text ? "Saving…" : "Saved");
    this.editorStatus.classList.toggle("editor-status-failure", reason === "import_failure");
    this.syncTextarea(null);
    const preview = this.controller.state.preview;
    this.previewButton.setAttribute("aria-pressed", String(preview));
    this.previewButton.textContent = preview ? "Edit" : "Preview";
    this.textarea.hidden = preview;
    this.preview.hidden = !preview;
    if (preview) this.preview.innerHTML = reason === "identity_conflict" ? ""
      : renderMarkdown(reason === "import_failure" ? editor.current.text : editor.value);
  }

  render(): void {
    const { snapshot, busy, error, showArchived, openDocumentId } = this.controller.state;
    if (this.composerDocumentId !== openDocumentId) {
      this.collapseEditorSelection();
      this.composerDocumentId = openDocumentId;
      this.draftQuote = null;
      this.replyTo = null;
    }
    this.status.textContent = AVAILABILITY_LABEL[snapshot.availability];
    this.archivedToggle.checked = showArchived;
    this.warnings.replaceChildren(
      ...(snapshot.warnings ?? []).map((text) => {
        const notice = document.createElement("p");
        notice.className = KIT.notice;
        notice.textContent = text;
        return notice;
      }),
    );
    this.error.hidden = error === null;
    this.error.textContent = error ?? "";
    this.collectionForm.submit.disabled = busy;
    const selected = this.controller.state.selectedCollectionId;
    if (selected !== this.selectedCollectionId) {
      this.selectedCollectionId = selected;
      if (selected) this.expanded.add(selected);
    }
    if (this.createTarget !== "collection" && this.createTarget !== selected) {
      if (this.createTarget !== null) this.createGeneration++;
      this.createTarget = null;
      this.documentForm.input.value = "";
    }
    this.documentForm.submit.disabled = busy || this.controller.selectedCollection === null;
    this.renderCollections();
    this.renderEditor();
    this.renderRails();
  }

  dispose(): void {
    this.resizeObserver?.disconnect();
    this.page.getRootNode().removeEventListener("click", this.onOutsideClick);
    this.page.getRootNode().removeEventListener("keydown", this.onEscape);
    document.removeEventListener("click", this.onOutsideClick);
    this.page.remove();
  }

  private renderRails(): void {
    this.rail.hidden = !this.leftOpen;
    this.leftHandle.hidden = !this.leftOpen;
    this.showNavigation.hidden = this.leftOpen;
    this.commentsToggle.setAttribute("aria-label", this.commentsOpen ? "Hide Comments" : "Show Comments");
    this.commentsToggle.title = this.commentsOpen ? "Hide Comments" : "Show Comments";
    this.commentsToggle.setAttribute("aria-pressed", String(this.commentsOpen));
    const showComments = this.commentsOpen && this.controller.openDocument !== null;
    this.commentsPanel.hidden = !showComments;
    this.rightHandle.hidden = !showComments;
    this.resizeRanges.forEach((update) => update());
  }

  private resizeHandle(side: "left" | "right"): HTMLElement {
    const handle = document.createElement("div");
    handle.className = `${side}-resizer documents-resizer`;
    handle.setAttribute("role", "separator");
    handle.setAttribute("aria-label", side === "left" ? "Resize navigation" : "Resize Comments");
    handle.setAttribute("aria-orientation", "vertical");
    handle.tabIndex = 0;
    let start = 0;
    let width = 0;
    const variable = side === "left" ? "--left-width" : "--right-width";
    const current = () => parseInt(this.shell.style.getPropertyValue(variable), 10) || (side === "left" ? 250 : 300);
    const range = () => {
      const shellWidth = this.shell.clientWidth;
      const cap = shellWidth <= 760 ? (side === "left" ? .7 : .75) : (side === "left" ? .32 : .36);
      const max = Math.max(0, Math.floor(Math.min(440, shellWidth * cap, shellWidth <= 760 ? 440 : shellWidth - 320)));
      return { min: Math.min(190, max), max };
    };
    const updateRange = () => {
      const { min, max } = range();
      handle.setAttribute("aria-valuemin", String(min));
      handle.setAttribute("aria-valuemax", String(max));
      handle.setAttribute("aria-valuenow", String(Math.max(min, Math.min(max, current()))));
    };
    const set = (value: number) => {
      const { min, max } = range();
      this.shell.style.setProperty(variable, `${Math.max(min, Math.min(max, value))}px`);
      updateRange();
    };
    handle.addEventListener("pointerdown", (event) => {
      updateRange();
      start = event.clientX;
      width = current();
      handle.setPointerCapture?.(event.pointerId);
    });
    handle.addEventListener("pointermove", (event) => {
      if (!handle.hasPointerCapture?.(event.pointerId)) return;
      set(width + (event.clientX - start) * (side === "left" ? 1 : -1));
    });
    handle.addEventListener("pointerup", (event) => {
      if (handle.hasPointerCapture?.(event.pointerId)) handle.releasePointerCapture(event.pointerId);
    });
    handle.addEventListener("keydown", (event) => {
      if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
      event.preventDefault();
      set(current() + (event.key === "ArrowRight" ? 20 : -20) * (side === "left" ? 1 : -1));
    });
    this.resizeRanges.push(updateRange);
    updateRange();
    return handle;
  }

  private dismissMenu(): void {
    this.activeMenu?.remove();
    this.activeMenuButton?.setAttribute("aria-expanded", "false");
    this.activeMenu = null;
    this.activeMenuButton = null;
  }

  private menu(row: HTMLElement, trigger: HTMLButtonElement, actions: HTMLElement): void {
    trigger.setAttribute("aria-expanded", "false");
    trigger.addEventListener("click", () => {
      if (this.activeMenuButton === trigger) { this.dismissMenu(); return; }
      this.dismissMenu();
      row.append(actions);
      this.activeMenu = actions;
      this.activeMenuButton = trigger;
      trigger.setAttribute("aria-expanded", "true");
      (actions.querySelector("button") as HTMLButtonElement | null)?.focus();
    });
  }

  private startRename(kind: "collection" | "document", id: string, value: string): void {
    this.dismissMenu();
    this.renameTarget = `${kind}:${id}`;
    const rename = inlineForm({
      label: kind === "collection" ? "Collection name" : "Document title",
      placeholder: kind === "collection" ? "Collection name" : "Document title",
      submitLabel: "Save", maxLength: kind === "collection" ? 128 : 256,
      className: kind === "collection" ? "collection-rename" : "document-rename",
      onSubmit: async (name) => {
        const ok = await (kind === "collection" ? this.controller.renameCollection(id, name) : this.controller.renameDocument(id, name));
        if (ok) { this.renameTarget = null; this.renameForm = null; }
        this.render();
        return ok;
      },
    });
    rename.input.value = value;
    rename.form.append(icon("x", "Cancel", () => {
      this.renameTarget = null;
      this.renameForm = null;
      this.render();
    }));
    this.renameForm = rename;
    this.render();
    rename.input.focus();
  }

  private renderCollections(): void {
    const root = this.collectionList.getRootNode() as ShadowRoot;
    const focused = root.activeElement;
    const inTree = focused !== null && this.collectionList.contains(focused);
    const focusKey = inTree ? (focused as HTMLElement).dataset.treeFocus
      ?? this.activeMenuButton?.dataset.treeFocus : undefined;
    const selection = inTree && focused instanceof HTMLInputElement
      ? { start: focused.selectionStart, end: focused.selectionEnd } : null;
    this.dismissMenu();
    const selected = this.controller.state.selectedCollectionId;
    const rows = this.controller.visibleCollections.map((collection) => {
      const item = document.createElement("li");
      item.className = "collection-group";
      const row = document.createElement("div");
      row.className = "tree-row";
      if (collection.archived) row.classList.add("is-archived");
      const expanded = this.expanded.has(collection.id);
      const expand = icon("chevron", `${expanded ? "Collapse" : "Expand"} ${collection.name}`, () => {
        if (expanded) this.expanded.delete(collection.id);
        else this.expanded.add(collection.id);
        this.render();
      });
      expand.dataset.treeFocus = `collection:expand:${collection.id}`;
      expand.setAttribute("aria-expanded", String(expanded));
      expand.classList.toggle("is-expanded", expanded);
      const select = button(collection.name, "collection-button", () => {
        if (this.controller.state.selectedCollectionId !== collection.id) this.expanded.add(collection.id);
        this.controller.selectCollection(collection.id);
      });
      select.dataset.treeFocus = `collection:select:${collection.id}`;
      if (collection.id === selected) select.setAttribute("aria-current", "page");
      const add = icon("plus", `Add Document to ${collection.name}`, () => {
        this.controller.selectCollection(collection.id);
        this.expanded.add(collection.id);
        this.createGeneration++;
        this.createTarget = collection.id;
        this.render();
        this.documentForm.input.focus();
      });
      const trigger = icon("more", `Actions for ${collection.name}`, () => {});
      add.dataset.treeFocus = `collection:add:${collection.id}`;
      trigger.dataset.treeFocus = `collection:actions:${collection.id}`;
      const actions = document.createElement("div");
      actions.className = "collection-actions action-menu";
      actions.setAttribute("role", "group");
      actions.setAttribute("aria-label", `Actions for ${collection.name}`);
      actions.append(
        button("Rename", KIT.secondary, () => this.startRename("collection", collection.id, collection.name)),
        button(collection.archived ? "Restore" : "Archive", KIT.secondary, () => {
          this.dismissMenu();
          void (collection.archived ? this.controller.restoreCollection(collection.id) : this.controller.archiveCollection(collection.id));
        }),
      );
      for (const control of actions.querySelectorAll("button")) control.disabled = this.controller.state.busy;
      this.menu(item, trigger, actions);
      row.append(expand, select, add, trigger);
      item.append(row);
      if (this.renameTarget === `collection:${collection.id}` && this.renameForm) item.append(this.renameForm.form);
      if (expanded) {
        const list = document.createElement("ul");
        list.className = "document-list";
        list.setAttribute("aria-label", `Documents in ${collection.name}`);
        const documents = this.controller.state.snapshot.documents.filter((d) =>
          d.collectionId === collection.id && (this.controller.state.showArchived || !d.archived));
        list.append(...documents.map((d) => this.documentRow(d, collection)));
        if (!documents.length) {
          const empty = document.createElement("li");
          empty.className = "documents-empty";
          empty.textContent = "No documents yet.";
          list.append(empty);
        }
        if (this.createTarget === collection.id) {
          const create = document.createElement("li");
          create.append(this.documentForm.form);
          list.append(create);
        }
        item.append(list);
      }
      return item;
    });
    this.collectionList.replaceChildren(...rows);
    if (this.createTarget === "collection") {
      const create = document.createElement("li");
      create.append(this.collectionForm.form);
      this.collectionList.prepend(create);
    }
    if (inTree) {
      const next = focused instanceof HTMLElement && this.collectionList.contains(focused) ? focused
        : [...this.collectionList.querySelectorAll<HTMLElement>("[data-tree-focus]")]
          .find((control) => control.dataset.treeFocus === focusKey);
      next?.focus();
      if (next === focused && selection && focused instanceof HTMLInputElement
        && selection.start !== null && selection.end !== null) {
        focused.setSelectionRange(selection.start, selection.end);
      }
    }
  }

  private documentRow(document_: Document, collection: Collection): HTMLLIElement {
    const row = document.createElement("li");
    row.className = "document-row";
    if (document_.archived) row.classList.add("is-archived");
    if (this.renameTarget === `document:${document_.id}` && this.renameForm) {
      row.append(this.renameForm.form);
      return row;
    }
    const title = button(document_.title + (document_.archived ? " (archived)" : ""), "document-title", () => {
      this.controller.selectCollection(collection.id);
      this.expanded.add(collection.id);
      void this.controller.openDocumentById(document_.id);
    });
    if (this.controller.state.openDocumentId === document_.id) title.setAttribute("aria-current", "true");
    const trigger = icon("more", `Actions for ${document_.title}`, () => {});
    title.dataset.treeFocus = `document:select:${document_.id}`;
    trigger.dataset.treeFocus = `document:actions:${document_.id}`;
    const actions = document.createElement("div");
    actions.className = "document-actions action-menu";
    actions.setAttribute("role", "group");
    actions.setAttribute("aria-label", `Actions for ${document_.title}`);
    actions.append(
      button("Rename", KIT.secondary, () => this.startRename("document", document_.id, document_.title)),
      button(document_.archived ? "Restore" : "Archive", KIT.secondary, () => {
        this.dismissMenu();
        void (document_.archived ? this.controller.restoreDocument(document_.id) : this.controller.archiveDocument(document_.id));
      }),
    );
    const destinations = this.controller.state.snapshot.collections.filter((c) => !c.archived && c.id !== collection.id);
    if (destinations.length > 0) {
      const move = document.createElement("select");
      move.className = `${KIT.input} document-move`;
      move.setAttribute("aria-label", `Move ${document_.title} to`);
      const placeholder = document.createElement("option");
      placeholder.value = "";
      placeholder.textContent = "Move to…";
      move.append(placeholder);
      for (const destination of destinations) {
        const option = document.createElement("option");
        option.value = destination.id;
        option.textContent = destination.name;
        move.append(option);
      }
      move.addEventListener("change", () => {
        if (move.value) { this.dismissMenu(); void this.controller.moveDocument(document_.id, move.value); }
      });
      move.disabled = this.controller.state.busy;
      actions.append(move);
    }
    for (const control of actions.querySelectorAll("button")) control.disabled = this.controller.state.busy;
    this.menu(row, trigger, actions);
    row.append(title, trigger);
    return row;
  }
}
