import { authorLabel, renderMarkdown, shortenedIdentity } from "./markdown";
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

function agentAttribution(description: string, tooltipId: string): HTMLElement {
  const wrapper = document.createElement("span");
  wrapper.className = "message-agent-attribution";
  const info = document.createElement("button");
  info.type = "button";
  info.className = "message-agent-info";
  info.setAttribute("aria-label", "About this Agent");
  info.setAttribute("aria-describedby", tooltipId);
  const icon = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  icon.setAttribute("viewBox", "0 0 24 24");
  icon.setAttribute("aria-hidden", "true");
  icon.setAttribute("focusable", "false");
  const circle = document.createElementNS(icon.namespaceURI, "circle");
  circle.setAttribute("cx", "12");
  circle.setAttribute("cy", "12");
  circle.setAttribute("r", "9");
  const detail = document.createElementNS(icon.namespaceURI, "path");
  detail.setAttribute("d", "M12 11v6m0-10h.01");
  icon.append(circle, detail);
  info.append(icon);
  const tooltip = document.createElement("span");
  tooltip.id = tooltipId;
  tooltip.className = "message-agent-tooltip";
  tooltip.setAttribute("role", "tooltip");
  tooltip.textContent = description;
  tooltip.hidden = true;
  let hovered = false;
  let focused = false;
  let dismissed = false;
  const update = (): void => { tooltip.hidden = dismissed || !(hovered || focused); };
  wrapper.addEventListener("mouseenter", () => { hovered = true; dismissed = false; update(); });
  wrapper.addEventListener("mouseleave", () => { hovered = false; update(); });
  info.addEventListener("focus", () => { focused = true; dismissed = false; update(); });
  info.addEventListener("blur", () => { focused = false; update(); });
  info.addEventListener("click", () => { dismissed = false; tooltip.hidden = false; });
  info.addEventListener("keydown", (event) => {
    if (event.key === "Escape") { dismissed = true; update(); event.stopPropagation(); }
  });
  wrapper.append(info, tooltip);
  return wrapper;
}

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
  private readonly historyStatus: HTMLElement;
  private readonly retryButton: HTMLButtonElement;
  private readonly feedRetryButton: HTMLButtonElement;
  private readonly pendingRetryButton: HTMLButtonElement;
  private readonly composer: HTMLFormElement;
  private readonly textarea: HTMLTextAreaElement;
  private readonly sendButton: HTMLButtonElement;
  private inputRevision = 0;
  private renderedChannelId: string | null = null;
  private renderedMessages: ControllerState["messages"] | null = null;
  private renderedPrincipals: ControllerState["principals"] | null = null;
  private renderedOldestId: string | null = null;
  private fillTimer: ReturnType<typeof setTimeout> | null = null;
  private resizeObserver: ResizeObserver | null = null;
  private observedWidth: number | null = null;
  private readonly observedRowHeights = new WeakMap<Element, number>();
  private readingAnchor: { id: string; offset: number; atBottom: boolean } | null = null;
  private disposed = false;
  private tooltipSequence = 0;

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
    this.history.addEventListener("scroll", this.onHistoryScroll);

    this.historyStatus = document.createElement("div");
    this.historyStatus.className = "history-status";
    this.historyStatus.setAttribute("aria-live", "polite");
    this.retryButton = document.createElement("button");
    this.retryButton.type = "button";
    this.retryButton.className = `${KIT_CLASS.secondary} retry-older`;
    this.retryButton.textContent = "Retry older history";
    this.retryButton.addEventListener("click", () => { void controller.retryOlder(); });
    this.feedRetryButton = document.createElement("button");
    this.feedRetryButton.type = "button";
    this.feedRetryButton.className = `${KIT_CLASS.secondary} retry-changes`;
    this.feedRetryButton.textContent = "Retry history updates";
    this.feedRetryButton.addEventListener("click", () => { void controller.retryFeed(); });
    this.pendingRetryButton = document.createElement("button");
    this.pendingRetryButton.type = "button";
    this.pendingRetryButton.className = `${KIT_CLASS.secondary} retry-pending`;
    this.pendingRetryButton.textContent = "Retry unavailable history";
    this.pendingRetryButton.addEventListener("click", () => { void controller.retryPending(); });
    this.panel.append(this.historyStatus, this.retryButton, this.feedRetryButton, this.pendingRetryButton);

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
      ++this.inputRevision;
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
      const channelId = controller.state.selectedChannelId;
      const revision = this.inputRevision;
      void controller.postMessage(markdown).then((acknowledged) => {
        if (this.disposed || !acknowledged || controller.state.selectedChannelId !== channelId
          || this.inputRevision !== revision || this.textarea.value !== markdown) return;
        this.textarea.value = "";
        controller.draft = "";
      });
    });

    this.panel.append(this.composer);
    this.shell.append(this.rail, this.panel);
    page.append(pageHeader, this.shell);
    root.replaceChildren(page);
    if (typeof ResizeObserver !== "undefined") {
      this.resizeObserver = new ResizeObserver(entries => {
        if (this.disposed) return;
        let reflowed = false;
        for (const entry of entries) {
          // The container changes width; a row can also change height at the same width
          // when a font finishes loading or its content reflows.
          const target = entry.target ?? this.history;
          if (target === this.history) {
            const width = entry.contentRect.width;
            if (this.observedWidth !== null && width !== this.observedWidth) reflowed = true;
            this.observedWidth = width;
          } else if (target instanceof HTMLElement && target.classList.contains("message")
            && this.history.contains(target)) {
            const height = entry.contentRect.height;
            const previous = this.observedRowHeights.get(target);
            if (previous !== undefined && height !== previous) reflowed = true;
            this.observedRowHeights.set(target, height);
          }
        }
        if (reflowed) this.restoreReadingAnchor();
        this.captureReadingAnchor();
      });
      this.resizeObserver.observe(this.history);
    }
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
    this.scheduleShortHistoryFill();

    // --- paging feedback ---
    this.retryButton.hidden = !state.olderError;
    this.retryButton.disabled = state.loadingOlder || state.retryingHead;
    this.feedRetryButton.hidden = !state.feedError;
    this.pendingRetryButton.hidden = state.pendingHistory === 0;
    this.pendingRetryButton.textContent = state.resolveError
      ? "Retry failed history access" : "Retry unavailable history";
    this.retryButton.textContent = !state.hasMore && state.messages.length === 0
      ? "Retry Channel history" : "Retry older history";
    this.historyStatus.textContent = state.loadingOlder
      ? "Loading older messages…"
      : state.retryingHead ? "Retrying Channel history…"
      : state.loading && state.selectedChannelId
        ? "Loading Channel history…"
      : state.feedError ? "History updates paused. Retry to resume."
      : state.resolveError ? "History access retry failed. Try again."
      : state.pendingHistory > 0 ? `${state.pendingHistory} Messages awaiting history access`
      : state.unavailableHistory > 0 ? `${state.unavailableHistory} Messages could not be opened`
      : "";

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
    this.history.removeEventListener("scroll", this.onHistoryScroll);
    this.resizeObserver?.disconnect();
    if (this.fillTimer !== null) clearTimeout(this.fillTimer);
    this.root.replaceChildren();
  }

  private scheduleShortHistoryFill(): void {
    if (this.fillTimer !== null || this.disposed) return;
    this.fillTimer = setTimeout(() => {
      this.fillTimer = null;
      const state = this.controller.state;
      if (!this.disposed && state.hasMore && !state.loading && !state.loadingOlder
        && !state.olderError && this.history.clientHeight > 0
        && this.history.scrollHeight <= this.history.clientHeight) {
        void this.controller.loadOlder();
      }
    }, 0);
  }

  private readonly onHistoryScroll = (): void => {
    this.captureReadingAnchor();
    const state = this.controller.state;
    if (!this.disposed && this.renderedChannelId === state.selectedChannelId
      && !state.loading && !state.loadingOlder && !state.olderError && state.hasMore
      && this.history.scrollTop <= 80) {
      void this.controller.loadOlder();
    }
  };

  private captureReadingAnchor(): void {
    const viewportTop = this.history.getBoundingClientRect().top;
    const first = [...this.history.querySelectorAll<HTMLElement>(".message")].find(row => {
      const rect = row.getBoundingClientRect();
      return rect.height > 0 && rect.bottom > viewportTop;
    });
    if (!first?.dataset.messageId) { this.readingAnchor = null; return; }
    this.readingAnchor = { id: first.dataset.messageId,
      offset: first.getBoundingClientRect().top - viewportTop,
      atBottom: this.history.scrollHeight - this.history.scrollTop - this.history.clientHeight < 40 };
  }

  private restoreReadingAnchor(): void {
    const anchor = this.readingAnchor;
    if (!anchor) return;
    if (anchor.atBottom) {
      this.history.scrollTop = this.history.scrollHeight;
      return;
    }
    const row = [...this.history.querySelectorAll<HTMLElement>(".message")]
      .find(candidate => candidate.dataset.messageId === anchor.id);
    if (row) this.history.scrollTop += row.getBoundingClientRect().top
      - this.history.getBoundingClientRect().top - anchor.offset;
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
    const focusedHide = selected?.id === this.renderedChannelId
      && this.headerSlot.querySelector("button") === (this.root instanceof ShadowRoot
        ? this.root.activeElement : document.activeElement);
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
    if (focusedHide) header.querySelector("button")?.focus({ preventScroll: true });
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
    if (this.renderedChannelId === state.selectedChannelId && this.renderedMessages === state.messages
      && this.renderedPrincipals === state.principals) return;
    this.renderedMessages = state.messages;
    this.renderedPrincipals = state.principals;
    // Messages read oldest at the top, newest at the bottom, and a Channel opens at the newest
    // one (TEAM.SYS.CONV-R08). Once a Member scrolls up, their position is kept.
    const openedChannel = state.selectedChannelId !== this.renderedChannelId;
    this.renderedChannelId = state.selectedChannelId;
    const previousTop = this.history.scrollTop;
    const previousHeight = this.history.scrollHeight;
    const previousRows = [...this.history.querySelectorAll<HTMLElement>(".message")];
    const focusedElement = this.root instanceof ShadowRoot ? this.root.activeElement : document.activeElement;
    const focusedAuthorId = !openedChannel && focusedElement instanceof HTMLElement
      && focusedElement.classList.contains("message-author")
      ? focusedElement.closest<HTMLElement>(".message")?.dataset.messageId : undefined;
    const focusedAgentInfoId = !openedChannel && focusedElement instanceof HTMLElement
      && focusedElement.classList.contains("message-agent-info")
      ? focusedElement.closest<HTMLElement>(".message")?.dataset.messageId : undefined;
    const focusedLinkRow = !openedChannel && focusedElement instanceof HTMLAnchorElement
      && this.history.contains(focusedElement)
      ? focusedElement.closest<HTMLElement>(".message") : null;
    const focusedLinkId = focusedLinkRow?.dataset.messageId;
    const focusedLinkIndex = focusedLinkRow
      ? [...focusedLinkRow.querySelectorAll<HTMLAnchorElement>(".message-markdown a")].indexOf(focusedElement as HTMLAnchorElement)
      : -1;
    const viewportTop = this.history.getBoundingClientRect().top;
    const visibleIndex = previousRows.findIndex(row => {
      const rect = row.getBoundingClientRect();
      return rect.height > 0 && rect.bottom > viewportTop;
    });
    const nextIds = new Set(state.messages.map(message => message.id));
    const survives = (row: HTMLElement): boolean =>
      row.dataset.messageId !== undefined && nextIds.has(row.dataset.messageId);
    const visibleSuccessor = visibleIndex < 0 ? null : previousRows.slice(visibleIndex)
      .find(row => survives(row) && row.getBoundingClientRect().top < viewportTop + this.history.clientHeight);
    const anchor = visibleSuccessor
      ?? (visibleIndex < 0 ? null : [...previousRows.slice(0, visibleIndex)].reverse().find(survives))
      ?? (visibleIndex < 0 ? null : previousRows.slice(visibleIndex).find(survives));
    const anchorTop = anchor?.getBoundingClientRect().top;
    const prepended = !openedChannel && this.renderedOldestId !== null
      && state.messages[0]?.id !== this.renderedOldestId
      && state.messages.some((message) => message.id === this.renderedOldestId);
    this.renderedOldestId = state.messages[0]?.id ?? null;
    const wasAtBottom =
      previousHeight - previousTop - this.history.clientHeight < 40;
    const newTailId = state.messages.at(-1)?.id;
    const tailAdvanced = newTailId !== undefined
      && !previousRows.some(row => row.dataset.messageId === newTailId);

    for (const row of previousRows) this.resizeObserver?.unobserve(row);
    this.history.replaceChildren();
    const principals = new Map(state.principals.principals.map(principal => [principal.identity, principal]));
    const counts = new Map<string, number>();
    for (const principal of state.principals.principals) {
      if (principal.name) counts.set(principal.name, (counts.get(principal.name) ?? 0) + 1);
    }
    let previousLocal = false;
    for (const message of state.messages) {
      const row = document.createElement("li");
      row.className = "message";
      row.dataset.messageId = message.id;
      if (message.isLocal) row.classList.add("message--local");
      if (message.isLocal && previousLocal) row.classList.add("message--local-continuation");
      previousLocal = message.isLocal;

      const author = document.createElement("button");
      author.type = "button";
      author.className = "message-author";
      const principal = principals.get(message.author);
      const name = principal?.name;
      author.textContent = authorLabel(message.author, undefined, message.isLocal,
        name, name !== null && name !== undefined && (counts.get(name) ?? 0) > 1);
      author.title = message.author;
      author.setAttribute(
        "aria-label",
        `Show full Public Identity ${message.author}`,
      );
      author.addEventListener("click", () => {
        author.textContent = message.author;
      });

      const authorLine = document.createElement("div");
      authorLine.className = "message-author-line";
      authorLine.append(author);
      if (!message.isLocal && principal?.kind === "agent") {
        const steward = state.principals.principals.find(candidate =>
          candidate.kind === "person" && candidate.principalId === principal.stewardPrincipalId);
        const description = `Agent · stewarded by ${steward?.name || shortenedIdentity(steward?.identity ?? principal.stewardPrincipalId ?? message.author)}`;
        authorLine.append(agentAttribution(description, `agent-attribution-${this.tooltipSequence++}`));
      }
      row.append(authorLine);

      const body = document.createElement("div");
      body.className = "message-markdown";
      body.innerHTML = renderMarkdown(message.markdown);

      row.append(body);
      this.history.append(row);
      this.resizeObserver?.observe(row);
      if (message.id === focusedAuthorId) author.focus({ preventScroll: true });
      if (message.id === focusedAgentInfoId) {
        row.querySelector<HTMLButtonElement>(".message-agent-info")?.focus({ preventScroll: true });
      }
      if (message.id === focusedLinkId && focusedLinkIndex >= 0) {
        row.querySelectorAll<HTMLAnchorElement>(".message-markdown a")[focusedLinkIndex]
          ?.focus({ preventScroll: true });
      }
    }

    if (openedChannel || (wasAtBottom && tailAdvanced)) {
      this.history.scrollTop = this.history.scrollHeight;
    } else if (anchor && anchorTop !== undefined) {
      const replacement = [...this.history.querySelectorAll<HTMLElement>(".message")]
        .find(row => row.dataset.messageId === anchor.dataset.messageId);
      this.history.scrollTop = replacement
        ? previousTop + replacement.getBoundingClientRect().top - anchorTop
        : previousTop;
    } else {
      // Geometry is unavailable in some test environments; preserve the old prepend fallback.
      this.history.scrollTop = previousTop + (prepended ? this.history.scrollHeight - previousHeight : 0);
    }
    this.captureReadingAnchor();
  }

  private patchComposer(state: ControllerState): void {
    // Sending is local-first: a Message is committed and synchronized later,
    // so only a Package that cannot read its own history yet may not send.
    const canSend =
      state.selectedChannelId !== null &&
      !state.loading &&
      !state.posting &&
      state.availability !== "waiting_for_history_access";
    this.sendButton.disabled = !canSend;
    this.textarea.placeholder = state.selectedChannelId
      ? "Write a message…"
      : "Select a Channel to write a message.";
  }
}