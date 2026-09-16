// @vitest-environment happy-dom

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
    }],
  };
}

function page(markdown: string): ConversationPage {
  return {
    messages: [{
      id: markdown,
      author: "bob",
      markdown,
    }],
  };
}

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
        processGeneration: 1,
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