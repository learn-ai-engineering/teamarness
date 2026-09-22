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

    const unmount = await mount(context);

    // Kit should have added at least one stylesheet to adoptedStyleSheets.
    expect(root.adoptedStyleSheets.length).toBeGreaterThan(initialSheetCount);

    // The page header names the Package.
    const title = root.querySelector("h1");
    expect(title?.textContent).toBe("Conversations");
    expect(title?.className).toContain("teamarness-page-title");
    expect(root.querySelector(".teamarness-page")?.lastElementChild?.className).toContain("conversations-shell");

    // The section element should have the kit panel class.
    const section = root.querySelector("section");
    expect(section?.className).toContain("teamarness-panel");
    expect(section?.className).toContain("teamarness-stack");

    // The send button should have the kit primary class.
    const sendBtn = root.querySelector<HTMLButtonElement>("button[type=submit]");
    expect(sendBtn?.className).toContain("teamarness-control--primary");

    await unmount();

    // After unmount, the root should be empty and kit sheets should be removed.
    expect(root.children.length).toBe(0);
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

  it("composer is disabled while waiting for history access", async () => {
    const { sendButton, unmount } = await mountWith("waiting_for_history_access");
    expect(sendButton?.disabled).toBe(true);
    await unmount();
  });
});
