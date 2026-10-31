// @vitest-environment happy-dom

import { createTestMountContext } from "@teamarness/package-ui-sdk/testing";
import { describe, expect, it } from "vitest";
import { mount } from "./index";
import { createFakeBacklog, order, recordId } from "./test-support/fake-backend";

describe("Backlog mount", () => {
  it("subscribes to changes, renders each snapshot, reports a stopped stream and unmounts cleanly", async () => {
    const host = document.createElement("div");
    document.body.append(host);
    const backlog = createFakeBacklog({ epics: [{ epicId: recordId(1), title: "Launch", archived: false, createdOrder: order(1) }] });
    const root = host.attachShadow({ mode: "open" });
    const unmount = await mount(createTestMountContext({ root, client: backlog.client, displayName: "Backlog" }));
    expect(backlog.fake.calls.map((call) => call.operation)).toEqual(["backlog.changes"]);
    expect(root.querySelector("h1")?.textContent).toBe("Backlog");
    expect(root.querySelector(".backlog-empty")?.textContent).toBe("Loading…");
    expect(root.querySelector("[role=status]")?.textContent).toBe("Catching up");

    await backlog.publish();
    expect(root.querySelector(".epic-title")?.textContent).toBe("Launch");
    expect(root.querySelector("[role=status]")?.textContent).toBe("Up to date");
    expect(root.querySelector("select")).toBeNull();

    backlog.fake.terminate("backlog.changes", null);
    expect((root.querySelector("[role=alert]") as HTMLElement).hidden).toBe(false);

    await unmount();
    expect(root.querySelector(".backlog-page")).toBeNull();
    host.remove();
  });

  it("says when there are no Epics yet", async () => {
    const backlog = createFakeBacklog();
    const root = document.createElement("div").attachShadow({ mode: "open" });
    const unmount = await mount(createTestMountContext({ root, client: backlog.client }));
    await backlog.publish();
    expect(root.querySelector(".backlog-empty")?.textContent).toBe("No Epics yet.");
    await unmount();
  });
});
