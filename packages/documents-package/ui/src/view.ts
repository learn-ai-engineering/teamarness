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

function button(label: string, className: string, onClick: () => void): HTMLButtonElement {
  const element = document.createElement("button");
  element.type = "button";
  element.className = className;
  element.textContent = label;
  element.addEventListener("click", onClick);
  return element;
}

/** A one-field form built once; the view only patches its disabled state. */
function inlineForm(options: {
  label: string;
  placeholder: string;
  submitLabel: string;
  maxLength: number;
  className: string;
  onSubmit: (value: string) => Promise<boolean>;
}): { form: HTMLFormElement; input: HTMLInputElement; submit: HTMLButtonElement } {
  const form = document.createElement("form");
  form.className = `${options.className} ${KIT.field}`;
  const input = document.createElement("input");
  input.className = KIT.input;
  input.type = "text";
  input.required = true;
  // HTML maxLength counts UTF-16 units, while the published limit counts characters.
  input.maxLength = options.maxLength * 2;
  input.addEventListener("input", () => input.setCustomValidity(""));
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
    void options.onSubmit(value).then((ok) => {
      if (ok) input.value = "";
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
  private readonly panelHeader: HTMLElement;
  private readonly documentList: HTMLUListElement;
  private readonly archivedToggle: HTMLInputElement;
  private readonly editorPane: HTMLElement;
  private readonly editorTitle: HTMLElement;
  private readonly editorStatus: HTMLElement;
  private readonly textarea: HTMLTextAreaElement;
  private readonly closeButton: HTMLButtonElement;
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
  private renamingDocumentId: string | null = null;
  private renamingCollection = false;

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
    const archivedLabel = document.createElement("label");
    archivedLabel.className = "archived-toggle";
    this.archivedToggle = document.createElement("input");
    this.archivedToggle.type = "checkbox";
    this.archivedToggle.addEventListener("change", () => controller.toggleArchived());
    archivedLabel.append(this.archivedToggle, document.createTextNode(" Show archived"));
    header.append(heading, this.status, archivedLabel);

    this.warnings = document.createElement("div");
    this.warnings.className = "documents-warnings";
    this.error = document.createElement("p");
    this.error.className = KIT.noticeDanger;
    this.error.setAttribute("role", "alert");
    this.error.hidden = true;

    const shell = document.createElement("div");
    shell.className = "documents-shell";

    const rail = document.createElement("aside");
    rail.className = "collection-rail";
    rail.setAttribute("aria-label", "Collections");
    const railHeading = document.createElement("h2");
    railHeading.textContent = "Collections";
    this.collectionList = document.createElement("ul");
    this.collectionList.className = "collection-list";
    this.collectionForm = inlineForm({
      label: "New Collection name",
      placeholder: "New Collection",
      submitLabel: "Add",
      maxLength: 128,
      className: "collection-create",
      onSubmit: (name) => controller.createCollection(name),
    });
    rail.append(railHeading, this.collectionList, this.collectionForm.form);

    const panel = document.createElement("section");
    panel.className = "documents-panel";
    this.panelHeader = document.createElement("div");
    this.panelHeader.className = "documents-panel-header";
    this.documentList = document.createElement("ul");
    this.documentList.className = "document-list";
    this.documentList.setAttribute("aria-label", "Documents");
    this.documentForm = inlineForm({
      label: "New Document title",
      placeholder: "New Document title",
      submitLabel: "Create",
      maxLength: 256,
      className: "document-create",
      onSubmit: (title) => controller.createDocument(title),
    });
    panel.append(this.panelHeader, this.documentList, this.documentForm.form);

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
    this.previewButton.setAttribute("aria-pressed", "false");
    this.closeButton = button("Close", KIT.secondary, () => { void controller.closeDocument(); });
    editorHeader.append(this.editorTitle, this.editorStatus, this.previewButton, this.closeButton);
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
    const quoteButton = button("Quote selection", KIT.secondary, () => {
      const start = this.textarea.selectionStart;
      const end = this.textarea.selectionEnd;
      const selected = this.textarea.value.slice(start, end).trim();
      this.draftQuote = selected ? selected.slice(0, 2048) : null;
      this.renderComposer();
    });
    this.commentInput = document.createElement("textarea");
    this.commentInput.className = `${KIT.input} comment-input`;
    this.commentInput.rows = 3;
    this.commentInput.maxLength = 32768;
    this.commentInput.placeholder = "Add a comment (Markdown)";
    this.commentInput.setAttribute("aria-label", "New comment");
    this.commentSubmit = document.createElement("button");
    this.commentSubmit.type = "submit";
    this.commentSubmit.className = KIT.primary;
    this.commentSubmit.textContent = "Comment";
    const composeActions = document.createElement("div");
    composeActions.className = "comment-compose-actions";
    composeActions.append(quoteButton, this.commentSubmit);
    this.commentForm.append(this.commentQuote, this.commentInput, composeActions);
    this.commentForm.addEventListener("submit", (event) => {
      event.preventDefault();
      const markdown = this.commentInput.value.trim();
      if (!markdown) return;
      void controller.addComment(markdown, this.draftQuote, this.replyTo).then((ok) => {
        if (ok) {
          this.commentInput.value = "";
          this.draftQuote = null;
          this.replyTo = null;
          this.renderComposer();
        }
      });
    });
    this.commentsPanel.append(commentsHeading, this.commentList, this.commentForm);
    this.editorPane.append(this.commentsPanel);

    shell.append(rail, panel, this.editorPane);
    this.page.append(header, this.warnings, this.error, shell);
    root.append(this.page);
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
    if (this.textarea.value !== editor.value) {
      this.textarea.value = editor.value;
      if (caret) this.textarea.setSelectionRange(caret.start, caret.end);
    }
  }

  private renderComposer(): void {
    this.commentQuote.hidden = this.draftQuote === null && this.replyTo === null;
    this.commentQuote.replaceChildren();
    if (this.replyTo !== null) {
      const reply = document.createElement("span");
      reply.className = "comment-reply-to";
      reply.textContent = "Replying to a comment";
      this.commentQuote.append(reply);
    }
    if (this.draftQuote !== null) {
      const quote = document.createElement("span");
      quote.textContent = this.draftQuote;
      this.commentQuote.append(quote);
    }
    if (!this.commentQuote.hidden) {
      this.commentQuote.append(button("Clear", KIT.secondary, () => {
        this.draftQuote = null;
        this.replyTo = null;
        this.renderComposer();
      }));
    }
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
    this.editorPane.hidden = open === null;
    if (open === null) return;
    this.renderComments();
    this.editorTitle.textContent = open.title;
    if (!editor) {
      this.editorStatus.textContent = "Opening…";
      this.textarea.disabled = true;
      return;
    }
    const { editable, sending, error } = editor.current;
    this.textarea.disabled = !editable;
    this.editorStatus.textContent = error ?? (!editable ? "Waiting for history" : sending || editor.value !== editor.current.text ? "Saving…" : "Saved");
    this.syncTextarea(null);
    const preview = this.controller.state.preview;
    this.previewButton.setAttribute("aria-pressed", String(preview));
    this.previewButton.textContent = preview ? "Edit" : "Preview";
    this.textarea.hidden = preview;
    this.preview.hidden = !preview;
    if (preview) this.preview.innerHTML = renderMarkdown(editor.value);
  }

  render(): void {
    const { snapshot, busy, error, showArchived } = this.controller.state;
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
    this.renderCollections();
    this.renderPanel();
    this.documentForm.submit.disabled = busy || this.controller.selectedCollection === null;
    this.documentForm.form.hidden = this.controller.selectedCollection === null;
    this.renderEditor();
  }

  dispose(): void {
    this.page.remove();
  }

  private renderCollections(): void {
    const selected = this.controller.state.selectedCollectionId;
    this.collectionList.replaceChildren(
      ...this.controller.visibleCollections.map((collection) => {
        const item = document.createElement("li");
        const select = button(collection.name, "collection-button", () => {
          this.renamingCollection = false;
          this.controller.selectCollection(collection.id);
        });
        if (collection.archived) select.classList.add("is-archived");
        if (collection.id === selected) select.setAttribute("aria-current", "page");
        item.append(select);
        return item;
      }),
    );
  }

  private renderPanel(): void {
    const collection = this.controller.selectedCollection;
    this.panelHeader.replaceChildren();
    if (collection === null) {
      const empty = document.createElement("p");
      empty.className = "documents-empty";
      empty.textContent = this.controller.state.snapshot.collections.length === 0
        ? "Create a Collection to start adding documents."
        : "Select a Collection.";
      this.panelHeader.append(empty);
      this.documentList.replaceChildren();
      return;
    }
    const busy = this.controller.state.busy;
    if (this.renamingCollection) {
      const rename = inlineForm({
        label: "Collection name",
        placeholder: "Collection name",
        submitLabel: "Save",
        maxLength: 128,
        className: "collection-rename",
        onSubmit: async (name) => {
          const ok = await this.controller.renameCollection(collection.id, name);
          if (ok) this.renamingCollection = false;
          this.render();
          return ok;
        },
      });
      rename.input.value = collection.name;
      rename.form.append(button("Cancel", KIT.secondary, () => { this.renamingCollection = false; this.render(); }));
      this.panelHeader.append(rename.form);
    } else {
      const name = document.createElement("h2");
      name.className = "collection-name";
      name.textContent = collection.name + (collection.archived ? " (archived)" : "");
      const actions = document.createElement("div");
      actions.className = "collection-actions";
      actions.append(
        button("Rename", KIT.secondary, () => { this.renamingCollection = true; this.render(); }),
        collection.archived
          ? button("Restore", KIT.secondary, () => { void this.controller.restoreCollection(collection.id); })
          : button("Archive", KIT.secondary, () => { void this.controller.archiveCollection(collection.id); }),
      );
      for (const control of actions.querySelectorAll("button")) control.disabled = busy;
      this.panelHeader.append(name, actions);
    }
    this.documentList.replaceChildren(...this.controller.visibleDocuments.map((d) => this.documentRow(d, collection)));
    if (this.controller.visibleDocuments.length === 0) {
      const empty = document.createElement("li");
      empty.className = "documents-empty";
      empty.textContent = "No documents yet.";
      this.documentList.append(empty);
    }
  }

  private documentRow(document_: Document, collection: Collection): HTMLLIElement {
    const row = document.createElement("li");
    row.className = "document-row";
    if (document_.archived) row.classList.add("is-archived");
    const busy = this.controller.state.busy;
    if (this.renamingDocumentId === document_.id) {
      const rename = inlineForm({
        label: "Document title",
        placeholder: "Document title",
        submitLabel: "Save",
        maxLength: 256,
        className: "document-rename",
        onSubmit: async (title) => {
          const ok = await this.controller.renameDocument(document_.id, title);
          if (ok) this.renamingDocumentId = null;
          this.render();
          return ok;
        },
      });
      rename.input.value = document_.title;
      rename.form.append(button("Cancel", KIT.secondary, () => { this.renamingDocumentId = null; this.render(); }));
      row.append(rename.form);
      return row;
    }
    const title = button(document_.title + (document_.archived ? " (archived)" : ""), "document-title", () => {
      void this.controller.openDocumentById(document_.id);
    });
    if (this.controller.state.openDocumentId === document_.id) title.setAttribute("aria-current", "true");
    const actions = document.createElement("div");
    actions.className = "document-actions";
    actions.append(
      button("Rename", KIT.secondary, () => { this.renamingDocumentId = document_.id; this.render(); }),
      document_.archived
        ? button("Restore", KIT.secondary, () => { void this.controller.restoreDocument(document_.id); })
        : button("Archive", KIT.secondary, () => { void this.controller.archiveDocument(document_.id); }),
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
        if (move.value) void this.controller.moveDocument(document_.id, move.value);
      });
      move.disabled = busy;
      actions.append(move);
    }
    for (const control of actions.querySelectorAll("button")) control.disabled = busy;
    row.append(title, actions);
    return row;
  }
}
