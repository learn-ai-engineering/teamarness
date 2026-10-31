// @vitest-environment happy-dom

import { createFakePackageClient, createTestMountContext } from "@teamarness/package-ui-sdk/testing";
import { describe, expect, it } from "vitest";
import { mount } from "./index";

describe("Backlog placeholder", () => {
  it("mounts a titled page and removes it on unmount", async () => {
    const host = document.createElement("div");
    document.body.append(host);
    const fake = createFakePackageClient();
    const context = createTestMountContext({ root: host.attachShadow({ mode: "open" }), client: fake.client });
    const unmount = await mount(context);
    expect(context.root.querySelector("h1")?.textContent).toBe("Backlog");
    expect(context.root.querySelector(".backlog-placeholder")).not.toBeNull();
    expect(context.root.querySelector("select")).toBeNull();
    await unmount();
    expect(context.root.querySelector(".backlog-page")).toBeNull();
    host.remove();
  });
});
