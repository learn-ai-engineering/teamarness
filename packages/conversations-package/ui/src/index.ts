import type { PackageMountContext, PackageUnmount } from "@teamarness/package-ui-sdk";
import { authorLabel, renderMarkdown } from "./markdown";
import "./styles.css";

export type ConversationChannel = Readonly<{
  id: string;
  slug: string;
  canHide: boolean;
}>;
export type ConversationMessage = Readonly<{
  id: string;
  author: string;
  markdown: string;
  isLocal?: boolean;
}>;
export type ConversationSnapshot = Readonly<{
  channels: readonly ConversationChannel[];
  availability?: string;
  warnings?: readonly string[];
}>;
export type ConversationPage = Readonly<{
  messages: readonly ConversationMessage[];
  nextBefore?: string;
}>;

type UiState = {
  snapshot: ConversationSnapshot;
  selectedChannel: string | null;
  messages: ConversationMessage[];
  nextBefore: string | null;
  hasMore: boolean;
  loading: boolean;
  loadingOlder: boolean;
  warning: string | null;
  creating: boolean;
  confirmingHide: boolean;
};

const PAGE_SIZE = 32;

function requestErrorCode(error: unknown): string {
  if (typeof error === "object" && error !== null && "code" in error) {
    const code = (error as { code?: unknown }).code;
    if (typeof code === "string" && code.length > 0) return code;
  }
  return "unknown_error";
}

export async function mount(context: PackageMountContext): Promise<PackageUnmount> {
  const state: UiState = {
    snapshot: { channels: [], availability: "waiting_for_history_access" },
    selectedChannel: null,
    messages: [],
    nextBefore: null,
    hasMore: false,
    loading: true,
    loadingOlder: false,
    warning: null,
    creating: false,
    confirmingHide: false,
  };
  let disposed = false;
  let render = (): void => undefined;

  const requestSnapshot = async (): Promise<void> => {
    const snapshot = await context.client.request<ConversationSnapshot>("conversations.snapshot", {});
    state.snapshot = snapshot;
    if (!state.selectedChannel || !snapshot.channels.some(({ id }) => id === state.selectedChannel)) {
      state.selectedChannel = snapshot.channels.find(({ slug }) => slug === "general")?.id ?? snapshot.channels[0]?.id ?? null;
      state.messages = [];
      state.nextBefore = null;
      state.hasMore = false;
    }
  };
  const requestPage = async (before?: string): Promise<ConversationPage> => {
    if (!state.selectedChannel) return { messages: [], nextBefore: undefined };
    const payload: Record<string, string | number> = { channelId: state.selectedChannel, limit: PAGE_SIZE };
    if (before) payload.before = before;
    return context.client.request<ConversationPage>("conversations.channel.page", payload);
  };
  const loadSelected = async (): Promise<void> => {
    state.loading = true;
    state.warning = null;
    render();
    try {
      const page = await context.client.request<ConversationPage>("conversations.channel.open", {
        channelId: state.selectedChannel,
        limit: PAGE_SIZE,
      });
      state.messages = [...page.messages];
      state.nextBefore = page.nextBefore ?? null;
      state.hasMore = page.nextBefore !== undefined;
    } catch {
      state.warning = "Conversation history is temporarily unavailable.";
    } finally {
      state.loading = false;
      render();
    }
  };
  const selectChannel = async (channelId: string): Promise<void> => {
    if (state.selectedChannel === channelId) return;
    state.selectedChannel = channelId;
    state.messages = [];
    state.nextBefore = null;
    state.hasMore = false;
    state.confirmingHide = false;
    await loadSelected();
  };
  const loadOlder = async (): Promise<void> => {
    if (!state.hasMore || !state.nextBefore || state.loadingOlder) return;
    state.loadingOlder = true;
    render();
    try {
      const page = await requestPage(state.nextBefore);
      state.messages = [...state.messages, ...page.messages];
      state.nextBefore = page.nextBefore ?? null;
      state.hasMore = page.nextBefore !== undefined;
    } catch {
      state.warning = "Older history could not be loaded.";
    } finally {
      state.loadingOlder = false;
      render();
    }
  };
  const createChannel = async (slug: string): Promise<void> => {
    state.loading = true;
    state.warning = null;
    render();
    try {
      await context.client.request("conversations.channel.create", { slug });
      state.creating = false;
      await requestSnapshot();
    } catch {
      state.warning = "Channel names use lowercase letters, numbers, and hyphens.";
    } finally {
      state.loading = false;
      render();
    }
  };
  const hideChannel = async (): Promise<void> => {
    if (!state.selectedChannel) return;
    state.loading = true;
    state.warning = null;
    render();
    try {
      await context.client.request("conversations.channel.hide", {
        channelId: state.selectedChannel,
        confirm: true,
      });
      state.confirmingHide = false;
      await requestSnapshot();
    } catch {
      state.warning = "Only the Channel creator can permanently hide this Channel.";
    } finally {
      state.loading = false;
      render();
    }
  };
  const postMessage = async (markdown: string): Promise<void> => {
    if (!state.selectedChannel || !markdown.trim()) return;
    state.loading = true;
    state.warning = null;
    render();
    try {
      await context.client.request("conversations.message.post", {
        channelId: state.selectedChannel,
        markdown,
      });
      await loadSelected();
    } catch (error) {
      const code = requestErrorCode(error);
      console.error("Conversations message post failed", { code, error });
      state.warning = `Message could not be saved (${code}).`;
      state.loading = false;
      render();
    }
  };

  render = (): void => {
    const available = state.snapshot.channels;
    const selected = available.find(({ id }) => id === state.selectedChannel) ?? null;
    const shell = document.createElement("div");
    shell.className = "conversations-shell";
    shell.dataset.narrowLayout = "true";

    const rail = document.createElement("aside");
    rail.className = "channel-rail";
    rail.setAttribute("aria-label", "Channels");
    const railHeading = document.createElement("h2");
    railHeading.textContent = "Channels";
    const channelList = document.createElement("ul");
    channelList.className = "channel-list";
    channelList.setAttribute("aria-label", "Conversation channels");
    for (const channel of available) {
      const item = document.createElement("li");
      const button = document.createElement("button");
      button.type = "button";
      button.className = "channel-button";
      button.dataset.channelId = channel.id;
      button.setAttribute("aria-pressed", String(channel.id === state.selectedChannel));
      button.textContent = `#${channel.slug}`;
      button.addEventListener("click", () => { void selectChannel(channel.id); });
      item.append(button);
      channelList.append(item);
    }
    const create = document.createElement("button");
    create.type = "button";
    create.className = "secondary-control";
    create.textContent = "Create Channel";
    create.addEventListener("click", () => {
      state.creating = !state.creating;
      render();
    });
    rail.append(railHeading, channelList, create);
    if (state.creating) {
      const createForm = document.createElement("form");
      createForm.className = "channel-create-form";
      const input = document.createElement("input");
      input.id = "new-channel-slug";
      input.name = "slug";
      input.required = true;
      input.maxLength = 48;
      input.pattern = "[a-z0-9-]+";
      input.placeholder = "channel-name";
      input.setAttribute("aria-label", "New Channel name");
      const submit = document.createElement("button");
      submit.type = "submit";
      submit.textContent = "Create";
      createForm.addEventListener("submit", (event) => {
        event.preventDefault();
        void createChannel(input.value);
      });
      createForm.append(input, submit);
      rail.append(createForm);
      queueMicrotask(() => input.focus());
    }

    const panel = document.createElement("section");
    panel.className = "conversation-panel";
    panel.setAttribute("aria-label", selected ? `#${selected.slug} conversation` : "Conversation");
    const header = document.createElement("header");
    header.className = "conversation-header";
    const heading = document.createElement("h2");
    heading.textContent = selected ? `#${selected.slug}` : "Conversations";
    header.append(heading);
    if (selected && selected.canHide) {
      const hide = document.createElement("button");
      hide.type = "button";
      hide.className = "danger-control";
      hide.textContent = "Hide Channel";
      hide.addEventListener("click", () => { state.confirmingHide = true; render(); });
      header.append(hide);
    }
    const status = document.createElement("p");
    status.className = "availability";
    status.setAttribute("role", "status");
    status.textContent = `Status: ${state.snapshot.availability ?? "current"}`;
    header.append(status);
    panel.append(header);

    if (state.confirmingHide) {
      const confirmation = document.createElement("div");
      confirmation.className = "hide-confirmation";
      confirmation.setAttribute("role", "alertdialog");
      confirmation.setAttribute("aria-label", "Confirm permanent Channel hide");
      const text = document.createElement("p");
      text.textContent = "Hiding this Channel is permanent. There is no restore path.";
      const confirm = document.createElement("button");
      confirm.type = "button";
      confirm.className = "danger-control";
      confirm.textContent = "Permanently Hide";
      confirm.addEventListener("click", () => { void hideChannel(); });
      const cancel = document.createElement("button");
      cancel.type = "button";
      cancel.className = "secondary-control";
      cancel.textContent = "Cancel";
      cancel.addEventListener("click", () => { state.confirmingHide = false; render(); });
      confirmation.append(text, confirm, cancel);
      panel.append(confirmation);
    }
    if (state.warning || state.snapshot.warnings?.length) {
      const warning = document.createElement("p");
      warning.className = "warning";
      warning.setAttribute("role", "alert");
      warning.textContent = state.warning ?? state.snapshot.warnings?.[0] ?? "Some records need attention.";
      panel.append(warning);
    }
    const history = document.createElement("ol");
    history.className = "message-history";
    history.setAttribute("aria-label", "Messages");
    for (const message of state.messages) {
      const row = document.createElement("li");
      row.className = "message";
      const author = document.createElement("button");
      author.type = "button";
      author.className = "message-author";
      author.textContent = authorLabel(message.author, undefined, message.isLocal);
      author.title = message.author;
      author.setAttribute("aria-label", `Show full Public Identity ${message.author}`);
      author.addEventListener("click", () => { author.textContent = message.author; });
      const body = document.createElement("div");
      body.className = "message-markdown";
      body.innerHTML = renderMarkdown(message.markdown);
      row.append(author, body);
      history.append(row);
    }
    panel.append(history);
    const older = document.createElement("button");
    older.type = "button";
    older.className = "secondary-control older-messages";
    older.textContent = state.loadingOlder ? "Loading older messages…" : "Load older messages";
    older.disabled = !state.hasMore || state.loadingOlder || state.loading;
    older.addEventListener("click", () => { void loadOlder(); });
    panel.append(older);

    const composer = document.createElement("form");
    composer.className = "composer";
    const label = document.createElement("label");
    label.htmlFor = "message-markdown";
    label.textContent = "Message (Markdown)";
    const textarea = document.createElement("textarea");
    textarea.id = "message-markdown";
    textarea.name = "markdown";
    textarea.required = true;
    textarea.maxLength = 32768;
    textarea.placeholder = "Write a message…";
    const send = document.createElement("button");
    send.type = "submit";
    send.textContent = "Send";
    send.disabled = !selected || state.loading || state.snapshot.availability === "offline";
    composer.addEventListener("submit", (event) => {
      event.preventDefault();
      void postMessage(textarea.value);
      textarea.value = "";
    });
    composer.append(label, textarea, send);
    panel.append(composer);
    shell.append(rail, panel);
    context.root.replaceChildren(shell);
  };

  await requestSnapshot();
  render();
  await loadSelected();
  const activeSubscription = await context.client.subscribe<ConversationSnapshot>("conversations.changes", {}, async (snapshot) => {
    state.snapshot = snapshot;
    await loadSelected();
  });

  return async () => {
    if (disposed) return;
    disposed = true;
    await activeSubscription.close();
    context.root.replaceChildren();
  };
}