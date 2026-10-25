// @vitest-environment happy-dom

import { createFakePackageClient, createTestMountContext } from "@teamarness/package-ui-sdk/testing";
import { describe, expect, it } from "vitest";
import { mount } from "./index";

describe("Exhibits placeholder mount", () => {
  it("lists Drafts newest first as the Backend returns them, then unmounts cleanly", async () => {
    const fake = createFakePackageClient({
      requests: {
        "exhibit.list": () => ({
          exhibits: [
            { id: "newer", title: "Newer", updatedAt: "2026-10-25T02:00:00.000Z", kind: "draft", revision: 2 },
            { id: "older", title: "Older", updatedAt: "2026-10-25T01:00:00.000Z", kind: "draft", revision: 1 },
          ],
        }),
      },
      subscriptions: { "exhibits.changed": () => undefined },
    });
    const root = document.createElement("div").attachShadow({ mode: "open" });
    const unmount = await mount(createTestMountContext({ client: fake.client, root, displayName: "Exhibits" }));
    expect(root.querySelector("h1")?.textContent).toBe("Exhibits");
    expect([...root.querySelectorAll(".exhibits-list li")].map((item) => item.textContent)).toEqual(["Newer", "Older"]);
    await unmount();
    expect(root.childElementCount).toBe(0);
  });

  it("shows an empty state", async () => {
    const fake = createFakePackageClient({
      requests: { "exhibit.list": () => ({ exhibits: [] }) },
      subscriptions: { "exhibits.changed": () => undefined },
    });
    const root = document.createElement("div").attachShadow({ mode: "open" });
    const unmount = await mount(createTestMountContext({ client: fake.client, root, displayName: "Exhibits" }));
    expect(root.querySelector(".exhibits-empty")?.textContent).toBe("No Exhibits yet.");
    await unmount();
  });
});
