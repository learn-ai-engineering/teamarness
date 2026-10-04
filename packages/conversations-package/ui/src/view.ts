import { authorLabel, renderMarkdown } from "./markdown";
import type { ConversationsController, ControllerState } from "./controller";

const KIT_CLASS = {
  control: "teamarness-control",
  primary: "teamarness-control teamarness-control--primary",
  secondary: "teamarness-control teamarness-control--secondary",
  field: "teamarness-field",
  input: "teamarness-input",
  notice: "teamarness-notice",
  noticeDanger: "teamarness-notice teamarness-notice--danger",
} as const;

/**
 * Builds the Conversations DOM once and updates regions in-place.
 * The composer (textarea + send button) is created during construction and
 * never replaced; only its disabled state, placeholder, and value are patched.
 */
export class ConversationsView {
  private readonly root: ShadowRoot | HTMLElement;
  private readonly controller: ConversationsController;
  private readonly shell: HTMLDivElement;
  private readonly rail: HTMLElement;
  private readonly panel: HTMLElement;
  private readonly headerSlot: HTMLElement;
  private readonly status: HTMLElement;
  private readonly channelList: HTMLUListElement;
  private readonly createButton: HTMLButtonElement;
  private readonly createArea: HTMLElement;
  private readonly history: HTMLElement;
  private readonly olderButton: HTMLButtonElement;
  private readonly composer: HTMLFormElement;
  private readonly textarea: HTMLTextAreaElement;
  private readonly sendButton: HTMLButtonElement;
  private renderedChannelId: string | null = null;
  private disposed = false;

  constructor(
    root: ShadowRoot | HTMLElement,
    controller: ConversationsController,
    title: string,
  ) {
    this.root = root;
    this.controller = controller;

    // --- page header ---
    const page = document.createElement("div");
    page.className = "teamarness-page conversations-page";
    const pageHeader = document.createElement("header");
    pageHeader.className = "teamarness-page-header";
    const pageTitle = document.createElement("h1");
    pageTitle.className = "teamarness-page-title";
    pageTitle.textContent = title;
    this.status = document.createElement("p");
    this.status.className = "availability";
    this.status.setAttribute("role", "status");
    pageHeader.append(pageTitle, this.status);

    // --- shell ---
    this.shell = document.createElement("div");
    this.shell.className = "conversations-shell";

    // --- channel rail ---
    this.rail = document.createElement("aside");
    this.rail.className = "channel-rail";
    this.rail.setAttribute("aria-label", "Channels");
    const channelHeading = document.createElement("div");
    channelHeading.className = "channel-heading";
    const heading = document.createElement("h2");
    heading.textContent = "Channels";
    this.createArea = document.createElement("div");
    this.createArea.className = "channel-create";
    this.createButton = document.createElement("button");
    this.createButton.type = "button";
    this.createButton.className = KIT_CLASS.secondary;
    this.createButton.textContent = "+";
    this.createButton.setAttribute("aria-label", "Create Channel");
    this.createButton.setAttribute("aria-expanded", "false");
    this.createButton.addEventListener("click", () => this.controller.toggleCreate());
    this.createArea.append(this.createButton);
    channelHeading.append(heading, this.createArea);
    this.channelList = document.createElement("ul");
    this.channelList.className = "channel-list";
    this.channelList.setAttribute("aria-label", "Conversation channels");
    this.rail.append(channelHeading, this.channelList);

    // --- conversation panel ---
    this.panel = document.createElement("section");
    this.panel.className = "conversation-panel";

    // --- header slot (replaced on every render) ---
    this.headerSlot = document.createElement("div");
    this.panel.append(this.headerSlot);

    // --- message history ---
    this.history = document.createElement("ul");
    this.history.className = "message-history";
    this.history.setAttribute("aria-label", "Messages");
    this.panel.append(this.history);

    // --- load-older button ---
    this.olderButton = document.createElement("button");
    this.olderButton.type = "button";
    this.olderButton.className = `${KIT_CLASS.secondary} older-messages`;
    this.olderButton.addEventListener("click", () => {
      void controller.loadOlder();
    });
    this.panel.append(this.olderButton);

    // --- composer (built once, never replaced) ---
    this.composer = document.createElement("form");
    this.composer.className = `composer ${KIT_CLASS.field}`;

    const label = document.createElement("label");
    label.htmlFor = "message-markdown";
    label.textContent = "Message (Markdown)";
    this.composer.append(label);

    this.textarea = document.createElement("textarea");
    this.textarea.id = "message-markdown";
    this.textarea.name = "markdown";
    this.textarea.className = KIT_CLASS.input;
    this.textarea.required = true;
    this.textarea.maxLength = 32768;
    this.textarea.placeholder = "Write a message…";
    this.textarea.addEventListener("input", () => {
      controller.draft = this.textarea.value;
    });
    this.composer.append(this.textarea);

    this.sendButton = document.createElement("button");
    this.sendButton.type = "submit";
    this.sendButton.className = KIT_CLASS.primary;
    this.sendButton.textContent = "Send";
    this.composer.append(this.sendButton);

    this.composer.addEventListener("submit", (event) => {
      event.preventDefault();
      const markdown = this.textarea.value;
      void controller.postMessage(markdown).then(() => {
        if (this.disposed) return;
        // Clear only on success (loading done + no warning).
        if (!controller.state.loading && !controller.state.warning) {
          this.textarea.value = "";
          controller.draft = "";
        }
      });
    });

    this.panel.append(this.composer);
    this.shell.append(this.rail, this.panel);
    page.append(pageHeader, this.shell);
    root.replaceChildren(page);
  }

  // ------------------------------------------------------------------
  // render
  // ------------------------------------------------------------------

  /** Update every region except the composer. */
  render(): void {
    if (this.disposed) return;
    const state = this.controller.state;

    // Resolve selected channel metadata.
    const selected = state.selectedChannelId
      ? (state.channels.find((c) => c.id === state.selectedChannelId) ?? null)
      : null;

    // --- rail ---
    this.renderRail(state);
    const labels = {
      waiting_for_history_access: "Waiting for history access",
      offline: "Sync unavailable",
      catching_up: "Checking for updates",
      current: "Up to date here",
    } as const;
    this.status.textContent = labels[state.availability];
    if (state.availability === "current") {
      this.status.setAttribute("aria-description", "Local known state; not a peer delivery receipt.");
    } else {
      this.status.removeAttribute("aria-description");
    }

    // --- header region ---
    this.renderHeader(state, selected);

    // --- warning / confirmation region ---
    this.renderWarnings(state);

    // --- history ---
    this.renderHistory(state);

    // --- older button ---
    this.olderButton.textContent = state.loadingOlder
      ? "Loading older messages…"
      : "Load older messages";
    this.olderButton.disabled =
      !state.hasMore || state.loadingOlder || state.loading;

    // --- composer (patch only; never recreate) ---
    this.patchComposer(state);

    // --- panel aria ---
    this.panel.setAttribute(
      "aria-label",
      selected ? `#${selected.slug} conversation` : "Conversation",
    );
  }

  // ------------------------------------------------------------------
  // lifecycle
  // ------------------------------------------------------------------

  dispose(): void {
    this.disposed = true;
    this.root.replaceChildren();
  }

  // ------------------------------------------------------------------
  // region renderers
  // ------------------------------------------------------------------

  private renderRail(state: ControllerState): void {
    const activeElement = this.root instanceof ShadowRoot
      ? this.root.activeElement
      : document.activeElement;
    const focusedChannel = this.channelList.contains(activeElement)
      ? (activeElement as HTMLElement).dataset.channelId
      : undefined;
    this.channelList.replaceChildren();
    for (const channel of state.channels) {
      const item = document.createElement("li");
      const button = document.createElement("button");
      button.type = "button";
      button.className = "channel-button";
      button.dataset.channelId = channel.id;
      button.setAttribute("aria-pressed", String(channel.id === state.selectedChannelId));
      if (channel.id === state.selectedChannelId) button.setAttribute("aria-current", "page");
      button.textContent = `#${channel.slug}`;
      button.addEventListener("click", () => {
        void this.controller.selectChannel(channel.id);
      });
      item.append(button);
      this.channelList.append(item);
      if (channel.id === focusedChannel) button.focus();
    }

    this.createButton.setAttribute("aria-expanded", String(state.creating));
    if (state.creating && !this.createArea.querySelector("form")) {
      const form = document.createElement("form");
      form.className = `channel-create-form ${KIT_CLASS.field}`;
      const input = document.createElement("input");
      input.id = "new-channel-slug";
      input.name = "slug";
      input.className = KIT_CLASS.input;
      input.required = true;
      input.maxLength = 48;
      input.pattern = "[a-z0-9-]+";
      input.placeholder = "channel-name";
      input.setAttribute("aria-label", "New Channel name");
      const submit = document.createElement("button");
      submit.type = "submit";
      submit.className = KIT_CLASS.primary;
      submit.textContent = "Create";
      form.addEventListener("submit", (event) => {
        event.preventDefault();
        void this.controller.createChannel(input.value);
      });
      form.addEventListener("keydown", (event) => {
        if (event.key === "Escape") {
          event.preventDefault();
          this.controller.toggleCreate();
          this.createButton.focus();
        }
      });
      form.append(input, submit);
      this.createArea.append(form);
      queueMicrotask(() => { if (form.isConnected) input.focus(); });
    } else if (!state.creating) {
      this.createArea.querySelector("form")?.remove();
    }
  }

  private renderHeader(
    state: ControllerState,
    selected: {
      id: string;
      slug: string;
      canHide: boolean;
    } | null,
  ): void {
    this.headerSlot.replaceChildren();
    const header = document.createElement("header");
    header.className = "conversation-header";

    const heading = document.createElement("h2");
    heading.textContent = selected ? `#${selected.slug}` : "Conversations";
    header.append(heading);

    if (selected && selected.canHide && !state.confirmingHide) {
      const hideBtn = document.createElement("button");
      hideBtn.type = "button";
      hideBtn.className = `${KIT_CLASS.secondary} teamarness-danger`;
      hideBtn.textContent = "Hide Channel…";
      hideBtn.addEventListener("click", () => {
        this.controller.showConfirmHide();
      });
      header.append(hideBtn);
    }

    this.headerSlot.append(header);
  }

  /** Renders warnings and the hide-confirmation dialog above the history. */
  private renderWarnings(state: ControllerState): void {
    const dialog = this.panel.querySelector(".hide-confirmation");
    const focusedDialogButton = dialog?.contains(document.activeElement)
      ? (document.activeElement as HTMLButtonElement).textContent
      : this.root instanceof ShadowRoot && dialog?.contains(this.root.activeElement)
        ? (this.root.activeElement as HTMLButtonElement).textContent
        : null;
    // Remove any existing warning / confirmation injected by a prior render.
    for (const el of this.panel.querySelectorAll(
      ".warning,.hide-confirmation",
    )) {
      el.remove();
    }

    // Hide confirmation dialog (place above history).
    if (state.confirmingHide) {
      const div = document.createElement("div");
      div.className = `${KIT_CLASS.noticeDanger} hide-confirmation`;
      div.setAttribute("role", "alertdialog");
      div.setAttribute("aria-label", "Confirm permanent Channel hide");

      const text = document.createElement("p");
      text.textContent =
        "Hiding this Channel is permanent. There is no restore path.";

      const confirmBtn = document.createElement("button");
      confirmBtn.type = "button";
      confirmBtn.className = `${KIT_CLASS.secondary} teamarness-danger`;
      confirmBtn.textContent = "Permanently Hide";
      confirmBtn.addEventListener("click", () => {
        void this.controller.hideChannel();
      });

      const cancelBtn = document.createElement("button");
      cancelBtn.type = "button";
      cancelBtn.className = KIT_CLASS.secondary;
      cancelBtn.textContent = "Cancel";
      cancelBtn.addEventListener("click", () => {
        this.controller.cancelConfirm();
        this.headerSlot.querySelector<HTMLButtonElement>("button")?.focus();
      });

      div.append(text, confirmBtn, cancelBtn);
      this.panel.insertBefore(div, this.history);
      if (focusedDialogButton === "Cancel") cancelBtn.focus();
      else if (!dialog || focusedDialogButton) confirmBtn.focus();
    }

    // Warning message.
    if (state.warning) {
      const el = document.createElement("p");
      el.className = `${KIT_CLASS.noticeDanger} warning`;
      el.setAttribute("role", "alert");
      el.textContent = state.warning;
      this.panel.insertBefore(el, this.history);
    } else if (!state.confirmingHide && state.warnings[0]) {
      const el = document.createElement("p");
      el.className = `${KIT_CLASS.noticeDanger} warning`;
      el.setAttribute("role", "alert");
      el.textContent = state.warnings[0];
      this.panel.insertBefore(el, this.history);
    }
  }

  private renderHistory(state: ControllerState): void {
    // Messages read oldest at the top, newest at the bottom, and a Channel opens at the newest
    // one (TEAM.SYS.CONV-R08). Once a Member scrolls up, their position is kept.
    const openedChannel = state.selectedChannelId !== this.renderedChannelId;
    this.renderedChannelId = state.selectedChannelId;
    const previousTop = this.history.scrollTop;
    const previousHeight = this.history.scrollHeight;
    const wasAtBottom =
      previousHeight - previousTop - this.history.clientHeight < 40;

    this.history.replaceChildren();
    let previousLocal = false;
    for (const message of state.messages) {
      const row = document.createElement("li");
      row.className = "message";
      if (message.isLocal) row.classList.add("message--local");
      if (message.isLocal && previousLocal) row.classList.add("message--local-continuation");
      previousLocal = message.isLocal;

      const author = document.createElement("button");
      author.type = "button";
      author.className = "message-author";
      author.textContent = authorLabel(message.author, undefined, message.isLocal);
      author.title = message.author;
      author.setAttribute(
        "aria-label",
        `Show full Public Identity ${message.author}`,
      );
      author.addEventListener("click", () => {
        author.textContent = message.author;
      });

      const body = document.createElement("div");
      body.className = "message-markdown";
      body.innerHTML = renderMarkdown(message.markdown);

      row.append(author, body);
      this.history.append(row);
    }

    if (openedChannel || wasAtBottom) {
      this.history.scrollTop = this.history.scrollHeight;
    } else {
      // Older Messages were added above: keep the Messages that were visible in place.
      this.history.scrollTop = previousTop + (this.history.scrollHeight - previousHeight);
    }
  }

  private patchComposer(state: ControllerState): void {
    // Sending is local-first: a Message is committed and synchronized later,
    // so only a Package that cannot read its own history yet may not send.
    const canSend =
      state.selectedChannelId !== null &&
      !state.loading &&
      state.availability !== "waiting_for_history_access";
    this.sendButton.disabled = !canSend;
    this.textarea.placeholder = state.selectedChannelId
      ? "Write a message…"
      : "Select a Channel to write a message.";
  }
}