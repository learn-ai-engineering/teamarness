// @vitest-environment happy-dom

import { createFakePackageClient, createTestMountContext } from "@teamarness/package-ui-sdk/testing";
import { describe, expect, it } from "vitest";
import type { DocumentsSnapshot } from "./index";
import { mount } from "./index";

function snapshot(): DocumentsSnapshot {
  return { collections: [], documents: [], availability: "current", warnings: [] };
}

describe("Documents mount", () => {
  it("renders the listing and availability, then unmounts cleanly", async () => {
    const fake = createFakePackageClient({
      requests: { "documents.snapshot": () => snapshot() },
      subscriptions: { "documents.changes": () => undefined },
    });
    const root = document.createElement("div").attachShadow({ mode: "open" });
    const unmount = await mount(createTestMountContext({ client: fake.client, root, displayName: "Documents" }));
    expect(root.querySelector("h1")?.textContent).toBe("Documents");
    expect(root.querySelector("[role=status]")?.textContent).toBe("Up to date");
    expect(root.querySelector(".documents-empty")?.textContent).toBe("No documents yet.");
    await unmount();
    expect(root.childElementCount).toBe(0);
  });
});
