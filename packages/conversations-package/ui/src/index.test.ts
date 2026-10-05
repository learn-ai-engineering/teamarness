// @vitest-environment happy-dom

import { TEAMARNESS_THEME_TOKENS } from "@teamarness/package-ui-sdk";
import { createFakePackageClient, createTestMountContext } from "@teamarness/package-ui-sdk/testing";
import type {
  PackageClient,
  PackageMountContext,
  PackageSubscription,
  PackageValue,
} from "@teamarness/package-ui-sdk";
import { describe, expect, it, vi } from "vitest";
import type { ConversationPage, ConversationSnapshot } from "./index";
import { mount } from "./index";

function snapshot(): ConversationSnapshot {
  return {
    availability: "current",
    channels: [{
      id: "00000000000000000000000000000000",
      slug: "general",
      canHide: false,
      unreadCount: 0,
    }],
  };
}

function page(markdown: string): ConversationPage {
  return { messages: [message(markdown)] };
}

function message(markdown: string): ConversationPage["messages"][number] {
  return { id: markdown, author: "bob", markdown, isLocal: false };
}

/** A page as the Backend returns one: newest Message first. */
function newestFirstPage(): ConversationPage {
  return { messages: [message("newest"), message("middle"), message("oldest")] };
}

function conversationsBackend() {
  return createFakePackageClient({
    requests: {
      "conversations.snapshot": () => snapshot(),
      "conversations.channel.open": () => newestFirstPage(),
    },
    subscriptions: { "conversations.changes": () => undefined },
  });
}

function shadowRoot(): ShadowRoot {
  return document.createElement("div").attachShadow({ mode: "open" });
}

describe("Conversations Package surface", () => {
  function backend(options: {
    availability?: ConversationSnapshot["availability"];
    messages?: ConversationPage["messages"];
    channels?: ConversationSnapshot["channels"];
    failCreate?: boolean;
    nextBefore?: string;
  } = {}) {
    const channels = options.channels ?? snapshot().channels;
    const availability = options.availability ?? "current";
    const client = createFakePackageClient({
      requests: {
        "conversations.snapshot": () => ({ availability, channels }),
        "conversations.channel.open": () => ({ messages: options.messages ?? [message("first")], ...(options.nextBefore ? { nextBefore: options.nextBefore } : {}) }),
        "conversations.channel.create": () => {
          if (options.failCreate) throw new Error("unavailable");
          return {};
        },
        "conversations.channel.page": () => ({ messages: [] }),
        "conversations.channel.hide": () => ({}),
      },
      subscriptions: { "conversations.changes": () => undefined },
    });
    return { ...client, update: () => client.emit("conversations.changes", { availability, channels }) };
  }

  it.each([
    ["waiting_for_history_access", "Waiting for history access"],
    ["offline", "Sync unavailable"],
    ["catching_up", "Checking for updates"],
    ["current", "Up to date here"],
  ] as const)("shows one Package-wide status for %s", async (availability, label) => {
    const root = shadowRoot();
    const unmount = await mount(createTestMountContext({ root, client: backend({ availability }).client }));
    const statuses = root.querySelectorAll('[role="status"]');
    expect(statuses).toHaveLength(1);
    const status = statuses.item(0)!;
    expect(root.querySelector(".teamarness-page-header")?.contains(status)).toBe(true);
    expect(status.textContent).toBe(label);
    expect(root.textContent).not.toContain(`Status: ${availability}`);
    if (availability === "current") expect(status.getAttribute("aria-description")).toMatch(/local|peer/i);
    await unmount();
  });

  it("shows an open transcript without fabricated times or numbered markers, with touching own accents", async () => {
    const root = shadowRoot();
    const messages = [
      { ...message("remote last"), id: "4" },
      { ...message("own second"), id: "3", isLocal: true },
      { ...message("own first"), id: "2", isLocal: true },
      { ...message("remote first"), id: "1" },
    ];
    const unmount = await mount(createTestMountContext({ root, client: backend({ messages }).client }));
    const history = root.querySelector(".message-history");
    const rows = [...root.querySelectorAll(".message")];
    expect(rows.map((row) => row.querySelector(".message-markdown")?.textContent?.trim())).toEqual([
      "remote first", "own first", "own second", "remote last",
    ]);
    expect(history?.tagName).not.toBe("OL");
    expect(rows.map((row) => row.classList.contains("message--local"))).toEqual([false, true, true, false]);
    expect(rows[2]?.classList.contains("message--local-continuation")).toBe(true);
    expect(rows[1]?.classList.contains("message--local-continuation")).toBe(false);
    expect(history?.querySelector("time,hr,.message-date,.message-number")).toBeNull();
    expect(root.querySelector(".older-messages")).toBeNull();
    await unmount();
  });

  it("keeps Channel selection and creation accessible beside the pinned heading", async () => {
    const host = document.createElement("div");
    document.body.append(host);
    const root = host.attachShadow({ mode: "open" });
    const channels = [snapshot().channels[0]!, { id: "second", slug: "team", canHide: true, unreadCount: 0 }];
    const source = backend({ channels });
    const unmount = await mount(createTestMountContext({ root, client: source.client }));
    const rail = root.querySelector(".channel-rail");
    const heading = rail?.querySelector(".channel-heading");
    const list = rail?.querySelector(".channel-list");
    expect(heading?.contains(list ?? null)).toBe(false);
    const create = heading?.querySelector<HTMLButtonElement>('button[aria-label="Create Channel"]');
    expect(create?.textContent).toBe("+");
    expect(root.querySelector('[data-channel-id="00000000000000000000000000000000"]')?.getAttribute("aria-current")).toBe("page");
    root.querySelector<HTMLButtonElement>('[data-channel-id="second"]')?.click();
    await vi.waitFor(() => expect(root.querySelector('[data-channel-id="second"]')?.getAttribute("aria-current")).toBe("page"));
    create?.click();
    await vi.waitFor(() => expect(root.activeElement).toBe(root.querySelector('[aria-label="New Channel name"]')));
    expect(heading?.querySelector("form")).not.toBeNull();
    const input = root.querySelector<HTMLInputElement>('[aria-label="New Channel name"]');
    input!.value = "valid-channel";
    input?.closest("form")?.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await vi.waitFor(() => expect(source.calls.filter((call) => call.operation === "conversations.channel.create")).toHaveLength(1));
    await vi.waitFor(() => expect(root.querySelector(".channel-create-form")).toBeNull());
    create?.click();
    await vi.waitFor(() => expect(root.activeElement).toBe(root.querySelector('[aria-label="New Channel name"]')));
    root.querySelector<HTMLInputElement>('[aria-label="New Channel name"]')?.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(root.querySelector(".channel-create-form")).toBeNull();
    expect(root.activeElement).toBe(create);
    await unmount();
    host.remove();
  });

  it("reports a Channel creation failure without closing the creation form", async () => {
    const root = shadowRoot();
    const source = backend({ failCreate: true });
    const unmount = await mount(createTestMountContext({ root, client: source.client }));
    root.querySelector<HTMLButtonElement>('[aria-label="Create Channel"]')?.click();
    const input = root.querySelector<HTMLInputElement>('[aria-label="New Channel name"]');
    input!.value = "valid-channel";
    input?.closest("form")?.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await vi.waitFor(() => expect(root.querySelector('[role="alert"]')?.textContent).toContain("Something went wrong"));
    expect(root.querySelector(".channel-create-form")).not.toBeNull();
    expect(source.calls.filter((call) => call.operation === "conversations.channel.create")).toHaveLength(1);
    await unmount();
  });

  it("does not show the normal older button in the successful paging state", async () => {
    const root = shadowRoot();
    const unmount = await mount(createTestMountContext({ root, client: backend({ nextBefore: "cursor" }).client }));
    expect(root.querySelector(".older-messages")).toBeNull();
    expect(root.querySelector<HTMLButtonElement>(".retry-older")?.hidden).toBe(true);
    await unmount();
  });

  it("requires confirmation for the top-right hide action and preserves the composer during updates", async () => {
    const host = document.createElement("div");
    document.body.append(host);
    const root = host.attachShadow({ mode: "open" });
    const channels = [{ id: snapshot().channels[0]!.id, slug: "general", canHide: true, unreadCount: 0 }];
    const source = backend({ channels });
    const unmount = await mount(createTestMountContext({ root, client: source.client }));
    const textarea = root.querySelector<HTMLTextAreaElement>("textarea");
    textarea!.value = "my draft";
    textarea!.dispatchEvent(new Event("input", { bubbles: true }));
    textarea!.focus();
    const hide = root.querySelector<HTMLButtonElement>(".conversation-header button");
    expect(hide?.textContent).toBe("Hide Channel…");
    expect(root.querySelector(".channel-rail")?.contains(hide ?? null)).toBe(false);
    hide?.click();
    expect(root.querySelector('[role="alertdialog"]')?.textContent).toContain("permanent");
    expect(root.querySelector('[role="alertdialog"] button')?.textContent).toBe("Permanently Hide");
    expect(root.activeElement).toBe(root.querySelector('[role="alertdialog"] button'));
    root.querySelector<HTMLButtonElement>('[role="alertdialog"] button:last-child')?.click();
    expect(root.activeElement).toBe(root.querySelector(".conversation-header button"));
    expect(source.calls.some((call) => call.operation === "conversations.channel.hide")).toBe(false);
    await source.update();
    expect(root.querySelector("textarea")).toBe(textarea);
    expect(textarea?.value).toBe("my draft");
    textarea?.focus();
    await source.update();
    expect(root.activeElement).toBe(textarea);
    await unmount();
    host.remove();
  });
});

describe("channel message order", () => {
  // Verifies: TEAM.SYS.CONV-R08
  it("renders the newest message at the bottom", async () => {
    const root = shadowRoot();
    const unmount = await mount(
      createTestMountContext({ root, client: conversationsBackend().client }),
    );

    const rendered = [...root.querySelectorAll(".message .message-markdown")]
      .map((element) => element.textContent?.trim());
    expect(rendered).toEqual(["oldest", "middle", "newest"]);

    await unmount();
  });

  // Verifies: TEAM.SYS.CONV-R08
  it("opens a channel scrolled to the bottom", async () => {
    // happy-dom performs no layout, so the test supplies the height the view scrolls to.
    const original = Object.getOwnPropertyDescriptor(Element.prototype, "scrollHeight");
    Object.defineProperty(Element.prototype, "scrollHeight", { configurable: true, get: () => 900 });
    try {
      const root = shadowRoot();
      const unmount = await mount(
        createTestMountContext({ root, client: conversationsBackend().client }),
      );

      const history = root.querySelector<HTMLElement>(".message-history");
      expect(history?.scrollTop).toBe(900);

      await unmount();
    } finally {
      if (original) Object.defineProperty(Element.prototype, "scrollHeight", original);
      else delete (Element.prototype as unknown as Record<string, unknown>).scrollHeight;
    }
  });
});

describe("conversation change delivery", () => {
  it("reloads the selected channel and does not render unread counters", async () => {
    let currentPage = page("initial message");
    let deliverChange: ((value: ConversationSnapshot) => void | Promise<void>) | undefined;
    const close = vi.fn(async () => undefined);
    const request = vi.fn(async (operation: string): Promise<PackageValue> => {
      if (operation === "conversations.snapshot") return snapshot();
      if (operation === "conversations.channel.open") return currentPage;
      throw new Error(`Unexpected operation: ${operation}`);
    });
    const client: PackageClient = {
      request: async <TResponse extends PackageValue>(operation: string) => (
        request(operation) as Promise<TResponse>
      ),
      subscribe: vi.fn(async (_operation, _payload, listener) => {
        deliverChange = listener as (value: ConversationSnapshot) => void | Promise<void>;
        return { close } satisfies PackageSubscription;
      }),
    };
    const host = document.createElement("div");
    const root = host.attachShadow({ mode: "open" });
    const context = {
      root,
      package: {
        identity: "dev.teamarness.conversations",
        version: "1.0.0",
        displayName: "Conversations",
        source: "bundled",
        trust: "application_bundled",
        lifecycle: "active",
        processGeneration: "1",
      },
      workspace: { id: "workspace", name: "Workspace" },
      uiHostVersion: 1,
      themeTokens: [],
      client,
    } satisfies PackageMountContext;

    const unmount = await mount(context);
    expect(root.textContent).toContain("initial message");

    currentPage = page("message delivered while general is open");
    await deliverChange?.(snapshot());

    expect(root.textContent).toContain("message delivered while general is open");
    expect(root.querySelector(".unread-count")).toBeNull();
    expect(request.mock.calls.filter(([operation]) => operation === "conversations.channel.open")).toHaveLength(2);

    await unmount();
    expect(close).toHaveBeenCalledOnce();
  });
});

describe("package UI kit integration", () => {
  it("removes both adopted sheets and the view if subscription fails", async () => {
    const root = shadowRoot();
    const initialSheets = root.adoptedStyleSheets.length;
    const source = conversationsBackend();
    const client: PackageClient = {
      request: source.client.request,
      subscribe: async () => { throw new Error("subscription unavailable"); },
    };
    await expect(mount(createTestMountContext({ root, client }))).rejects.toThrow("subscription unavailable");
    expect(root.adoptedStyleSheets).toHaveLength(initialSheets);
    expect(root.children).toHaveLength(0);
    const unmount = await mount(createTestMountContext({ root, client: source.client }));
    expect(root.adoptedStyleSheets).toHaveLength(initialSheets + 2);
    await unmount();
    expect(root.adoptedStyleSheets).toHaveLength(initialSheets);
  });

  it("installs the shared UI kit on mount and cleans it up on unmount", async () => {
    const close = vi.fn(async () => undefined);
    const request = vi.fn(async (operation: string): Promise<PackageValue> => {
      if (operation === "conversations.snapshot") return snapshot();
      if (operation === "conversations.channel.open") return page("kit test");
      throw new Error(`Unexpected operation: ${operation}`);
    });
    const client: PackageClient = {
      request: async <TResponse extends PackageValue>(operation: string) => (
        request(operation) as Promise<TResponse>
      ),
      subscribe: vi.fn(async () => ({ close })),
    };
    const host = document.createElement("div");
    const root = host.attachShadow({ mode: "open" });
    const initialSheetCount = root.adoptedStyleSheets.length;
    const context = {
      root,
      package: {
        identity: "dev.teamarness.conversations",
        version: "1.0.0",
        displayName: "Conversations",
        source: "bundled",
        trust: "application_bundled",
        lifecycle: "active",
        processGeneration: "1",
      },
      workspace: { id: "workspace", name: "Workspace" },
      uiHostVersion: 1,
      themeTokens: TEAMARNESS_THEME_TOKENS,
      client,
    } satisfies PackageMountContext;

    // happy-dom does not expose parsed cssRules for constructed stylesheets.
    const cssBySheet = new WeakMap<CSSStyleSheet, string>();
    const replaceSync = CSSStyleSheet.prototype.replaceSync;
    const replaceSpy = vi.spyOn(CSSStyleSheet.prototype, "replaceSync").mockImplementation(function (this: CSSStyleSheet, css) {
      cssBySheet.set(this, css);
      replaceSync.call(this, css);
    });
    try {
      const unmount = await mount(context);

      // Both the UI kit and Conversations layout must be adopted into this ShadowRoot.
      expect(root.adoptedStyleSheets.length).toBe(initialSheetCount + 2);
      const layoutSheet = root.adoptedStyleSheets.at(-1)!;
      expect(cssBySheet.get(layoutSheet)).toMatch(/\.conversations-shell\s*\{[^}]*display:\s*grid/);
      expect(root.querySelector("style,link[rel=stylesheet]")).toBeNull();

      // The page header names the Package.
      const title = root.querySelector("h1");
      expect(title?.textContent).toBe("Conversations");
      expect(title?.className).toContain("teamarness-page-title");
      expect(root.querySelector(".teamarness-page")?.lastElementChild?.className).toContain("conversations-shell");

      // The Package Page fills the frame without an enclosing kit panel.
      const section = root.querySelector("section");
      expect(root.querySelector(".teamarness-page")).not.toBeNull();
      expect(section?.classList.contains("teamarness-panel")).toBe(false);
      expect(section?.classList.contains("teamarness-stack")).toBe(false);

      // The send button should have the kit primary class.
      const sendBtn = root.querySelector<HTMLButtonElement>("button[type=submit]");
      expect(sendBtn?.className).toContain("teamarness-control--primary");

      await unmount();

      // Unmount removes the layout and kit sheets, not only the DOM.
      expect(root.children.length).toBe(0);
      expect(root.adoptedStyleSheets).toHaveLength(initialSheetCount);
    } finally {
      replaceSpy.mockRestore();
    }
  });

  it("receives theme tokens from the Shell", async () => {
    const close = vi.fn(async () => undefined);
    const request = vi.fn(async (operation: string): Promise<PackageValue> => {
      if (operation === "conversations.snapshot") return snapshot();
      if (operation === "conversations.channel.open") return page("theme test");
      throw new Error(`Unexpected operation: ${operation}`);
    });
    const client: PackageClient = {
      request: async <TResponse extends PackageValue>(operation: string) => (
        request(operation) as Promise<TResponse>
      ),
      subscribe: vi.fn(async () => ({ close })),
    };
    const host = document.createElement("div");
    const root = host.attachShadow({ mode: "open" });
    const context = {
      root,
      package: {
        identity: "dev.teamarness.conversations",
        version: "1.0.0",
        displayName: "Conversations",
        source: "bundled",
        trust: "application_bundled",
        lifecycle: "active",
        processGeneration: "1",
      },
      workspace: { id: "workspace", name: "Workspace" },
      uiHostVersion: 1,
      // Provide the full set of 12 theme tokens as the Shell would.
      themeTokens: TEAMARNESS_THEME_TOKENS,
      client,
    } satisfies PackageMountContext;

    const unmount = await mount(context);

    // The host element should be in component form.
    expect(root.textContent).toContain("#general");

    await unmount();
  });
});
describe("composer acknowledgement ownership", () => {
  it.each(["failure", "success"])("preserves B's draft after a late A post %s", async (outcome) => {
    const channels = [snapshot().channels[0]!, { id: "second", slug: "second", canHide: false, unreadCount: 0 }];
    let resolvePost!: (value: PackageValue) => void;
    let rejectPost!: (error: Error) => void;
    const client: PackageClient = {
      request: <T extends PackageValue>(operation: string): Promise<T> => {
        if (operation === "conversations.snapshot") return Promise.resolve({ ...snapshot(), channels }) as unknown as Promise<T>;
        if (operation === "conversations.channel.open") return Promise.resolve(page("fixture")) as unknown as Promise<T>;
        if (operation === "conversations.message.post") return new Promise((resolve, reject) => {
          resolvePost = (value) => resolve(value as T);
          rejectPost = reject;
        });
        throw new Error(`Unexpected operation: ${operation}`);
      },
      subscribe: async () => ({ close: async () => undefined }),
    };
    const root = shadowRoot();
    const unmount = await mount(createTestMountContext({ root, client }));
    const textarea = root.querySelector<HTMLTextAreaElement>("textarea")!;
    textarea.value = "draft A";
    textarea.dispatchEvent(new Event("input", { bubbles: true }));
    textarea.closest("form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    root.querySelector<HTMLButtonElement>('[data-channel-id="second"]')!.click();
    await vi.waitFor(() => expect(root.querySelector('[data-channel-id="second"]')?.getAttribute("aria-current")).toBe("page"));
    textarea.value = "draft B";
    textarea.dispatchEvent(new Event("input", { bubbles: true }));
    if (outcome === "failure") rejectPost(new Error("post failed"));
    else resolvePost({ ok: true });
    await vi.waitFor(() => expect(textarea.value).toBe("draft B"));
    // Let the submit continuation run before asserting its effect.
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(textarea.value).toBe("draft B");
    await unmount();
  });

  it.each([false, true])("clears a successful post only when its draft has not been edited (edited: %s)", async (edited) => {
    let resolvePost!: (value: PackageValue) => void;
    const client: PackageClient = {
      request: <T extends PackageValue>(operation: string): Promise<T> => {
        if (operation === "conversations.snapshot") return Promise.resolve(snapshot()) as unknown as Promise<T>;
        if (operation === "conversations.channel.open") return Promise.resolve(page("fixture")) as unknown as Promise<T>;
        if (operation === "conversations.message.post") return new Promise((resolve) => { resolvePost = (value) => resolve(value as T); });
        throw new Error(`Unexpected operation: ${operation}`);
      },
      subscribe: async () => ({ close: async () => undefined }),
    };
    const root = shadowRoot();
    const unmount = await mount(createTestMountContext({ root, client }));
    const textarea = root.querySelector<HTMLTextAreaElement>("textarea")!;
    textarea.value = "submitted";
    textarea.closest("form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    if (edited) {
      textarea.value = "edited while pending";
      textarea.dispatchEvent(new Event("input", { bubbles: true }));
    }
    resolvePost({ ok: true });
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(textarea.value).toBe(edited ? "edited while pending" : "");
    expect(root.querySelector<HTMLButtonElement>(".composer button")?.disabled).toBe(false);
    await unmount();
  });
});

describe("composer availability", () => {
  async function mountWith(availability: string) {
    const close = vi.fn(async () => undefined);
    const request = vi.fn(async (operation: string): Promise<PackageValue> => {
      if (operation === "conversations.snapshot") {
        return { ...snapshot(), availability };
      }
      if (operation === "conversations.channel.open") return page("availability test");
      throw new Error(`Unexpected operation: ${operation}`);
    });
    const client: PackageClient = {
      request: async <TResponse extends PackageValue>(operation: string) => (
        request(operation) as Promise<TResponse>
      ),
      subscribe: vi.fn(async () => ({ close })),
    };
    const host = document.createElement("div");
    const root = host.attachShadow({ mode: "open" });
    const context = {
      root,
      package: {
        identity: "dev.teamarness.conversations",
        version: "1.0.0",
        displayName: "Conversations",
        source: "bundled",
        trust: "application_bundled",
        lifecycle: "active",
        processGeneration: "1",
      },
      workspace: { id: "workspace", name: "Workspace" },
      uiHostVersion: 1,
      themeTokens: [],
      client,
    } satisfies PackageMountContext;
    const unmount = await mount(context);
    const sendButton = root.querySelector<HTMLButtonElement>("button[type=submit]");
    return { sendButton, unmount };
  }

  it("composer stays enabled while offline", async () => {
    const { sendButton, unmount } = await mountWith("offline");
    expect(sendButton?.disabled).toBe(false);
    await unmount();
  });

  // Verifies: TEAM.SYS.TRANS-R08, TEAM.SYS.CONV-R14
  it("composer is disabled while waiting for history access", async () => {
    const { sendButton, unmount } = await mountWith("waiting_for_history_access");
    expect(sendButton?.disabled).toBe(true);
    await unmount();
  });
});

describe("automatic older history in the Message container", () => {
  it("continues past an empty advancing page while already at the top of scrollable history", async () => {
    const replies: Array<(page: ConversationPage) => void> = [];
    const calls: PackageValue[] = [];
    const client: PackageClient = {
      request: <T extends PackageValue>(operation: string, payload: PackageValue): Promise<T> => {
        if (operation === "conversations.snapshot") return Promise.resolve(snapshot()) as unknown as Promise<T>;
        if (operation === "conversations.channel.open") return Promise.resolve({ messages: [message("newest")], nextBefore: "first" }) as unknown as Promise<T>;
        if (operation === "conversations.channel.page") {
          calls.push(payload);
          return new Promise((resolve) => { replies.push((page) => resolve(page as unknown as T)); });
        }
        throw new Error(`Unexpected operation: ${operation}`);
      },
      subscribe: async () => ({ close: async () => undefined }),
    };
    const root = shadowRoot();
    const unmount = await mount(createTestMountContext({ root, client }));
    const history = root.querySelector<HTMLElement>(".message-history")!;
    Object.defineProperty(history, "clientHeight", { get: () => 100 });
    Object.defineProperty(history, "scrollHeight", { get: () => 900 });
    history.scrollTop = 0;
    history.dispatchEvent(new Event("scroll"));
    expect(calls).toHaveLength(1);
    replies[0]!({ messages: [], nextBefore: "second" });
    await vi.waitFor(() => expect(calls).toHaveLength(2));
    expect(calls[1]).toEqual({ channelId: snapshot().channels[0]!.id, before: "second", limit: 32 });
    replies[1]!({ messages: [message("older")] });
    await vi.waitFor(() => expect(history.querySelectorAll(".message")).toHaveLength(2));
    expect(root.querySelector(".history-exhausted")?.textContent).toContain("Beginning");
    await unmount();
  });

  it.each(["initial", "switch"])("shows a keyboard-operable retry, not exhaustion, after %s open failure", async (mode) => {
    const channels = [snapshot().channels[0]!, { id: "second", slug: "second", canHide: false, unreadCount: 0 }];
    let failures = 1;
    const client: PackageClient = {
      request: <T extends PackageValue>(operation: string, payload: PackageValue): Promise<T> => {
        if (operation === "conversations.snapshot") return Promise.resolve({ ...snapshot(), channels }) as unknown as Promise<T>;
        if (operation === "conversations.channel.open") {
          if ((mode === "initial" || (payload as { channelId: string }).channelId === "second") && failures-- > 0) return Promise.reject(new Error("open failed"));
          return Promise.resolve({ messages: [] }) as unknown as Promise<T>;
        }
        throw new Error(`Unexpected operation: ${operation}`);
      },
      subscribe: async () => ({ close: async () => undefined }),
    };
    const host = document.createElement("div");
    document.body.append(host);
    const root = host.attachShadow({ mode: "open" });
    const unmount = await mount(createTestMountContext({ root, client }));
    if (mode === "switch") {
      root.querySelector<HTMLButtonElement>('[data-channel-id="second"]')!.click();
      await vi.waitFor(() => expect(root.querySelector('[data-channel-id="second"]')?.getAttribute("aria-current")).toBe("page"));
      await vi.waitFor(() => expect(root.querySelector(".warning")).not.toBeNull());
    }
    expect(root.querySelector(".history-status")?.textContent).not.toContain("Beginning");
    const retry = root.querySelector<HTMLButtonElement>(".retry-older")!;
    expect(retry.hidden).toBe(false);
    expect(retry.disabled).toBe(false);
    expect(retry.textContent).toBe("Retry Channel history");
    retry.focus();
    expect(root.activeElement).toBe(retry);
    retry.click();
    await vi.waitFor(() => expect(root.querySelector(".history-status")?.textContent).toContain("Beginning"));
    expect(retry.hidden).toBe(true);
    await unmount();
    host.remove();
  });

  function scrollBackend() {
    let resolvePage!: (page: ConversationPage) => void;
    let rejectPage!: (error: Error) => void;
    const requests: { operation: string; payload: PackageValue }[] = [];
    const client: PackageClient = {
      request: <T extends PackageValue>(operation: string, payload: PackageValue): Promise<T> => {
        requests.push({ operation, payload });
        if (operation === "conversations.snapshot") return Promise.resolve(snapshot()) as unknown as Promise<T>;
        if (operation === "conversations.channel.open") return Promise.resolve({ messages: [message("newest"), message("oldest")], nextBefore: "cursor" }) as unknown as Promise<T>;
        if (operation === "conversations.channel.page") return new Promise((resolve, reject) => {
          resolvePage = (page) => resolve(page as unknown as T);
          rejectPage = reject;
        });
        throw new Error(`Unexpected operation: ${operation}`);
      },
      subscribe: async () => ({ close: async () => undefined }),
    };
    return { client, requests, resolve: (page: ConversationPage) => resolvePage(page), reject: () => rejectPage(new Error("fixture failure")) };
  }

  it("loads only near the history top, one request at a time, anchors on prepend and stops at exhaustion", async () => {
    const source = scrollBackend();
    const root = shadowRoot();
    const unmount = await mount(createTestMountContext({ root, client: source.client }));
    const history = root.querySelector<HTMLElement>(".message-history")!;
    let heightPerRow = 100;
    Object.defineProperty(history, "clientHeight", { configurable: true, get: () => 100 });
    Object.defineProperty(history, "scrollHeight", { configurable: true, get: () => history.querySelectorAll(".message").length * heightPerRow });
    history.scrollTop = 110;
    history.dispatchEvent(new Event("scroll"));
    root.querySelector(".channel-list")?.dispatchEvent(new Event("scroll"));
    expect(source.requests.filter((r) => r.operation === "conversations.channel.page")).toHaveLength(0);
    history.scrollTop = 12;
    history.dispatchEvent(new Event("scroll"));
    history.dispatchEvent(new Event("scroll"));
    expect(source.requests.filter((r) => r.operation === "conversations.channel.page")).toHaveLength(1);
    expect(root.querySelector(".older-messages")).toBeNull();
    source.resolve({ messages: [message("older")], nextBefore: "end" });
    await vi.waitFor(() => expect(history.querySelectorAll(".message")).toHaveLength(3));
    expect(history.scrollTop).toBe(112);
    heightPerRow = 120;
    history.scrollTop = 5;
    history.dispatchEvent(new Event("scroll"));
    source.resolve({ messages: [] });
    await vi.waitFor(() => expect(root.querySelector(".history-exhausted")).not.toBeNull());
    history.dispatchEvent(new Event("scroll"));
    expect(source.requests.filter((r) => r.operation === "conversations.channel.page")).toHaveLength(2);
    await unmount();
  });

  it("does not auto-loop on short initial history or failure, and offers a keyboard retry", async () => {
    const source = scrollBackend();
    const root = shadowRoot();
    const unmount = await mount(createTestMountContext({ root, client: source.client }));
    const history = root.querySelector<HTMLElement>(".message-history")!;
    Object.defineProperty(history, "clientHeight", { get: () => 500 });
    Object.defineProperty(history, "scrollHeight", { get: () => 200 });
    expect(source.requests.filter((r) => r.operation === "conversations.channel.page")).toHaveLength(0);
    history.dispatchEvent(new Event("scroll"));
    source.reject();
    await vi.waitFor(() => expect(root.querySelector<HTMLButtonElement>(".retry-older")?.disabled).toBe(false));
    for (let i = 0; i < 3; i++) history.dispatchEvent(new Event("scroll"));
    expect(source.requests.filter((r) => r.operation === "conversations.channel.page")).toHaveLength(1);
    const retry = root.querySelector<HTMLButtonElement>(".retry-older")!;
    expect(retry.textContent).toMatch(/retry older history/i);
    retry.click();
    expect(source.requests.filter((r) => r.operation === "conversations.channel.page")).toHaveLength(2);
    source.resolve({ messages: [] });
    await vi.waitFor(() => expect(root.querySelector(".history-exhausted")).not.toBeNull());
    await unmount();
    history.dispatchEvent(new Event("scroll"));
    expect(source.requests.filter((r) => r.operation === "conversations.channel.page")).toHaveLength(2);
  });
});

describe("live Message scroll position", () => {
  it("follows a new Message only from the bottom, preserving draft and a reader above it", async () => {
    let messages = [message("second"), message("first")];
    const source = createFakePackageClient({
      requests: {
        "conversations.snapshot": () => snapshot(),
        "conversations.channel.open": () => ({ messages }),
      },
      subscriptions: { "conversations.changes": () => undefined },
    });
    const root = shadowRoot();
    const unmount = await mount(createTestMountContext({ root, client: source.client }));
    const history = root.querySelector<HTMLElement>(".message-history")!;
    let rowHeight = 100;
    Object.defineProperty(history, "clientHeight", { get: () => 100 });
    Object.defineProperty(history, "scrollHeight", { get: () => history.querySelectorAll(".message").length * rowHeight });
    const textarea = root.querySelector<HTMLTextAreaElement>("textarea")!;
    textarea.value = "fixture draft";
    history.scrollTop = 5;
    messages = [message("third"), ...messages];
    await source.emit("conversations.changes", snapshot());
    expect(history.scrollTop).toBe(5);
    expect(textarea.value).toBe("fixture draft");
    history.scrollTop = 260;
    rowHeight = 120;
    messages = [message("fourth"), ...messages];
    await source.emit("conversations.changes", snapshot());
    expect(history.scrollTop).toBe(480);
    await unmount();
  });
});

describe("short initial history", () => {
  it("fills a non-scrollable history without looping on a pending page", async () => {
    const originalHeight = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "scrollHeight");
    const originalClient = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "clientHeight");
    Object.defineProperty(HTMLElement.prototype, "scrollHeight", { configurable: true, get() {
      return (this as Element).querySelectorAll(".message").length * 100;
    } });
    Object.defineProperty(HTMLElement.prototype, "clientHeight", { configurable: true, get: () => 300 });
    try {
      let resolvePage!: (page: ConversationPage) => void;
      const calls: string[] = [];
      const client: PackageClient = {
        request: <T extends PackageValue>(operation: string): Promise<T> => {
          calls.push(operation);
          if (operation === "conversations.snapshot") return Promise.resolve(snapshot()) as unknown as Promise<T>;
          if (operation === "conversations.channel.open") return Promise.resolve({ messages: [message("newest")], nextBefore: "older" }) as unknown as Promise<T>;
          return new Promise((resolve) => { resolvePage = (page) => resolve(page as unknown as T); });
        },
        subscribe: async () => ({ close: async () => undefined }),
      };
      const root = shadowRoot();
      const unmount = await mount(createTestMountContext({ root, client }));
      expect(root.querySelector(".message-history")?.querySelectorAll(".message")).toHaveLength(1);
      expect(root.querySelector(".message-history")?.clientHeight).toBe(300);
      expect(root.querySelector(".message-history")?.scrollHeight).toBe(100);
      await vi.waitFor(() => expect(calls.filter((call) => call === "conversations.channel.page")).toHaveLength(1));
      await new Promise((resolve) => setTimeout(resolve, 40));
      expect(calls.filter((call) => call === "conversations.channel.page")).toHaveLength(1);
      resolvePage({ messages: [message("older"), message("oldest")] });
      await vi.waitFor(() => expect(root.querySelector(".history-exhausted")).not.toBeNull());
      await unmount();
    } finally {
      if (originalHeight) Object.defineProperty(HTMLElement.prototype, "scrollHeight", originalHeight);
      else delete (HTMLElement.prototype as unknown as Record<string, unknown>).scrollHeight;
      if (originalClient) Object.defineProperty(HTMLElement.prototype, "clientHeight", originalClient);
      else delete (HTMLElement.prototype as unknown as Record<string, unknown>).clientHeight;
    }
  });
});

describe("Channel scroll reset", () => {
  it("opens a different Channel at its newest Message", async () => {
    const channels = [snapshot().channels[0]!, { id: "second", slug: "second", canHide: false, unreadCount: 0 }];
    const source = createFakePackageClient({
      requests: {
        "conversations.snapshot": () => ({ ...snapshot(), channels }),
        "conversations.channel.open": (payload) => ({ messages: [message((payload as { channelId: string }).channelId)] }),
      },
      subscriptions: { "conversations.changes": () => undefined },
    });
    const root = shadowRoot();
    const unmount = await mount(createTestMountContext({ root, client: source.client }));
    const history = root.querySelector<HTMLElement>(".message-history")!;
    Object.defineProperty(history, "scrollHeight", { get: () => 800 });
    Object.defineProperty(history, "clientHeight", { get: () => 100 });
    history.scrollTop = 15;
    root.querySelector<HTMLButtonElement>('[data-channel-id="second"]')?.click();
    await vi.waitFor(() => expect(history.querySelector(".message-markdown")?.textContent?.trim()).toBe("second"));
    expect(history.scrollTop).toBe(800);
    await unmount();
  });
});
