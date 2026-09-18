import { PACKAGE_REQUEST_ERROR_KIND } from "@teamarness/package-ui-sdk";
import type { PackageClient, PackageValue } from "@teamarness/package-ui-sdk";
import { describe, expect, it } from "vitest";
import { ConversationsController } from "./controller";

/** A rejected Package request as the frame bridge brands it. */
function packageRequestError(code: string, retryable: boolean): unknown {
  return Object.freeze({
    kind: PACKAGE_REQUEST_ERROR_KIND,
    version: 2,
    namespace: "teamarness.package-host",
    code: "backend_error",
    stage: "ui",
    retryable,
    recovery: "none",
    diagnosticId: "diag-1",
    packageError: Object.freeze({ code, retryable, details: Object.freeze({}) }),
  });
}

const CHANNEL = "00000000000000000000000000000000";
const OTHER_CHANNEL = "11111111111111111111111111111111";

function client(rejection: unknown): PackageClient {
  return {
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
        channels: [{ id: CHANNEL, slug: "general", canHide: false }],
      } as unknown as TResponse;
    },
    subscribe: async () => ({ close: async () => undefined }),
  };
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
