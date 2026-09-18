import { typedPackageClient } from "@teamarness/package-ui-sdk";
import type { PackageClient, PackageValue } from "@teamarness/package-ui-sdk";
import { createPackageRequestError } from "@teamarness/package-ui-sdk/testing";
import { describe, expect, it } from "vitest";
import type { ConversationsContract } from "./contract.generated";
import { ConversationsController, type ConversationsClient } from "./controller";

/** A rejected Package request as the frame bridge brands it. */
function packageRequestError(code: string, retryable: boolean): unknown {
  return createPackageRequestError({
    stage: "ui",
    retryable,
    diagnosticId: "diag-1",
    packageError: { code, retryable },
  });
}

const CHANNEL = "00000000000000000000000000000000";
const OTHER_CHANNEL = "11111111111111111111111111111111";

function client(rejection: unknown): ConversationsClient {
  const untyped: PackageClient = {
    request: async <TResponse extends PackageValue>(
      operation: string,
      payload: PackageValue,
    ): Promise<TResponse> => {
      if (operation === "conversations.channel.open") {
        const channelId = (payload as { channelId?: string }).channelId;
        if (channelId === OTHER_CHANNEL) throw rejection;
        return { messages: [] } as unknown as TResponse;
      }
      return {
        availability: "current",
        channels: [{ id: CHANNEL, slug: "general", canHide: false, unreadCount: 0 }],
      } as unknown as TResponse;
    },
    subscribe: async () => ({ close: async () => undefined }),
  };
  return typedPackageClient<ConversationsContract>(untyped);
}

/** Reaches an active, `current` session, then fails one channel open. */
async function failingOpen(rejection: unknown): Promise<ConversationsController> {
  const controller = new ConversationsController(client(rejection), () => undefined);
  await controller.initialize();
  await controller.selectChannel(OTHER_CHANNEL);
  return controller;
}

describe("Conversations error classification", () => {
  it("uses the Package code and retryability from a typed error", async () => {
    const controller = await failingOpen(packageRequestError("conversations.channel_hidden", false));
    expect(controller.state.warning).toBe("This Channel has been permanently hidden.");
  });

  it("keeps the retryable message for a retryable Package error", async () => {
    const controller = await failingOpen(packageRequestError("conversations.channel_hidden", true));
    expect(controller.state.warning).toBe("Unable to complete the action. Please try again.");
  });

  it("falls back to the host code when there is no Package error", async () => {
    const controller = await failingOpen({ code: "stale_mount", retryable: false });
    expect(controller.state.warning).toBe("Something went wrong. Please try again.");
  });
});
