// @vitest-environment happy-dom

import {
  createFakePackageClient,
  createPackageRequestError,
  createTestMountContext,
  type FakePackageClient,
} from "@teamarness/package-ui-sdk/testing";
import type { PackageUnmount, PackageValue } from "@teamarness/package-ui-sdk";
import { afterEach, describe, expect, it } from "vitest";
import { mount } from "./index";
import type { Page, Summary } from "./contract.generated";

const NEWER: Summary = { id: "newer", title: "Newer", updatedAt: "2026-10-25T02:00:00.000Z", kind: "draft", revision: 2 };
const OLDER: Summary = { id: "older", title: "Older", updatedAt: "2026-10-25T01:00:00.000Z", kind: "draft", revision: 1 };

type Harness = {
  readonly fake: FakePackageClient;
  readonly root: ShadowRoot;
  readonly unmount: PackageUnmount;
  /** The listing `exhibit.list` answers next. */
  listing: Page;
  /** The revision `exhibit.open` reports next, per Exhibit. */
  revisions: Map<string, number>;
};

const unmounts: PackageUnmount[] = [];

afterEach(async () => {
  for (const unmount of unmounts.splice(0)) await unmount();
});

async function settle(): Promise<void> {
  for (let turn = 0; turn < 10; turn += 1) await Promise.resolve();
  await new Promise((resolve) => setTimeout(resolve, 0));
}

async function mountExhibits(input?: {
  listing?: Page;
  list?: (payload: PackageValue) => Page | Promise<Page>;
  open?: (exhibitId: string, revision: number) => { url: string; revision: number } | Promise<{ url: string; revision: number }>;
}): Promise<Harness> {
  const state = {
    listing: input?.listing ?? { exhibits: [NEWER, OLDER] },
    revisions: new Map<string, number>([[NEWER.id, NEWER.revision], [OLDER.id, OLDER.revision]]),
  };
  const fake = createFakePackageClient({
    requests: {
      "exhibit.list": (payload) => (input?.list ? input.list(payload) : state.listing),
      "exhibit.open": (payload) => {
        const exhibitId = (payload as { exhibitId: string }).exhibitId;
        const revision = state.revisions.get(exhibitId) ?? 1;
        if (input?.open) return input.open(exhibitId, revision);
        return { url: `teamarness-content://localhost/${exhibitId}-r${revision}/index.html`, revision };
      },
    },
    subscriptions: { "exhibits.changed": () => undefined },
  });
  const root = document.createElement("div").attachShadow({ mode: "open" });
  const unmount = await mount(createTestMountContext({ client: fake.client, root, displayName: "Exhibits" }));
  unmounts.push(unmount);
  await settle();
  return Object.assign(state, { fake, root, unmount });
}

function listedTitles(root: ShadowRoot): string[] {
  return [...root.querySelectorAll(".exhibits-list .exhibits-item-title")].map((item) => item.textContent ?? "");
}

function itemButton(root: ShadowRoot, title: string): HTMLButtonElement {
  const button = [...root.querySelectorAll<HTMLButtonElement>(".exhibits-list button")]
    .find((candidate) => candidate.querySelector(".exhibits-item-title")?.textContent === title);
  if (!button) throw new Error(`no listed Exhibit titled ${title}`);
  return button;
}

function frame(root: ShadowRoot): HTMLIFrameElement | null {
  return root.querySelector("iframe");
}

function calls(fake: FakePackageClient, operation: string): PackageValue[] {
  return fake.calls.filter((call) => call.operation === operation).map((call) => call.payload);
}

async function select(harness: Harness, title: string): Promise<void> {
  itemButton(harness.root, title).click();
  await settle();
}

describe("Exhibits list", () => {
  it("lists Drafts newest first, as the Backend orders them", async () => {
    const { root, fake } = await mountExhibits();
    expect(root.querySelector("h1")?.textContent).toBe("Exhibits");
    expect(listedTitles(root)).toEqual(["Newer", "Older"]);
    expect(calls(fake, "exhibit.list")).toEqual([{}]);
    expect(frame(root)).toBeNull();
  });

  it("shows an empty state", async () => {
    const { root } = await mountExhibits({ listing: { exhibits: [] } });
    expect(root.querySelector(".exhibits-empty")?.textContent).toBe("No Exhibits yet.");
    expect(root.querySelector(".exhibits-list")).toBeNull();
  });

  it("shows an error state when the list cannot be read, and recovers on retry", async () => {
    let fail = true;
    const { root } = await mountExhibits({
      list: () => {
        if (fail) throw createPackageRequestError({ code: "backend_unavailable" });
        return { exhibits: [NEWER] };
      },
    });
    const alert = root.querySelector('[role="alert"]');
    expect(alert?.textContent).toContain("Exhibits could not be loaded");
    expect(listedTitles(root)).toEqual([]);
    fail = false;
    root.querySelector<HTMLButtonElement>(".exhibits-retry")?.click();
    await settle();
    expect(root.querySelector('[role="alert"]')).toBeNull();
    expect(listedTitles(root)).toEqual(["Newer"]);
  });

  it("loads older Exhibits after the first page", async () => {
    const { root, fake } = await mountExhibits({
      list: (payload) => ((payload as { before?: string }).before === "cursor-1"
        ? { exhibits: [OLDER] }
        : { exhibits: [NEWER], nextBefore: "cursor-1" }),
    });
    expect(listedTitles(root)).toEqual(["Newer"]);
    root.querySelector<HTMLButtonElement>(".exhibits-older")?.click();
    await settle();
    expect(calls(fake, "exhibit.list")).toEqual([{}, { before: "cursor-1" }]);
    expect(listedTitles(root)).toEqual(["Newer", "Older"]);
    expect(root.querySelector(".exhibits-older")).toBeNull();
  });
});

describe("Exhibits viewer", () => {
  it("opens a selected Draft in a frame sandboxed to scripts only", async () => {
    const harness = await mountExhibits();
    await select(harness, "Older");
    expect(calls(harness.fake, "exhibit.open")).toEqual([{ exhibitId: "older" }]);
    const view = frame(harness.root);
    expect(view?.getAttribute("sandbox")).toBe("allow-scripts");
    expect(view?.getAttribute("src")).toBe("teamarness-content://localhost/older-r1/index.html");
    expect(view?.getAttribute("title")).toBe("Exhibit: Older");
    expect(itemButton(harness.root, "Older").getAttribute("aria-current")).toBe("true");
    expect(itemButton(harness.root, "Newer").hasAttribute("aria-current")).toBe(false);
  });

  it("shows an error when a Draft cannot be opened", async () => {
    const harness = await mountExhibits({
      open: () => {
        throw createPackageRequestError({ packageError: { code: "exhibits.not_found" } });
      },
    });
    await select(harness, "Newer");
    expect(frame(harness.root)).toBeNull();
    expect(harness.root.querySelector('.exhibits-viewer [role="alert"]')?.textContent)
      .toContain("This Exhibit could not be opened");
  });

  it("keeps the latest selection when an earlier open answers late", async () => {
    let releaseFirst: (() => void) | undefined;
    const harness = await mountExhibits({
      open: async (exhibitId, revision) => {
        if (exhibitId === "newer") await new Promise<void>((resolve) => { releaseFirst = resolve; });
        return { url: `teamarness-content://localhost/${exhibitId}/index.html`, revision };
      },
    });
    itemButton(harness.root, "Newer").click();
    await settle();
    await select(harness, "Older");
    releaseFirst?.();
    await settle();
    expect(frame(harness.root)?.getAttribute("src")).toBe("teamarness-content://localhost/older/index.html");
  });

  it("re-opens the open Draft and swaps the frame source when its revision moves", async () => {
    const harness = await mountExhibits();
    await select(harness, "Newer");
    const view = frame(harness.root);
    harness.revisions.set("newer", 3);
    harness.listing = { exhibits: [{ ...NEWER, revision: 3, updatedAt: "2026-10-25T03:00:00.000Z" }, OLDER] };
    await harness.fake.emit("exhibits.changed", { exhibitId: "newer", revision: 3, deleted: false });
    await settle();
    expect(calls(harness.fake, "exhibit.open")).toEqual([{ exhibitId: "newer" }, { exhibitId: "newer" }]);
    expect(frame(harness.root)).toBe(view);
    expect(view?.getAttribute("src")).toBe("teamarness-content://localhost/newer-r3/index.html");
    expect(view?.getAttribute("sandbox")).toBe("allow-scripts");
  });

  it("does not re-open the open Draft for another Draft's change, but refreshes the list", async () => {
    const harness = await mountExhibits();
    await select(harness, "Older");
    const added: Summary = { id: "added", title: "Added", updatedAt: "2026-10-25T04:00:00.000Z", kind: "draft", revision: 1 };
    harness.listing = { exhibits: [added, NEWER, OLDER] };
    await harness.fake.emit("exhibits.changed", { exhibitId: "added", revision: 1, deleted: false });
    await settle();
    expect(calls(harness.fake, "exhibit.open")).toEqual([{ exhibitId: "older" }]);
    expect(frame(harness.root)?.getAttribute("src")).toBe("teamarness-content://localhost/older-r1/index.html");
    expect(listedTitles(harness.root)).toEqual(["Added", "Newer", "Older"]);
  });

  it("does not re-open for a change at or below the revision already shown", async () => {
    const harness = await mountExhibits();
    await select(harness, "Newer");
    await harness.fake.emit("exhibits.changed", { exhibitId: "newer", revision: 2, deleted: false });
    await settle();
    expect(calls(harness.fake, "exhibit.open")).toEqual([{ exhibitId: "newer" }]);
  });

  it("closes the viewer when the open Draft is deleted", async () => {
    const harness = await mountExhibits();
    await select(harness, "Newer");
    harness.listing = { exhibits: [OLDER] };
    await harness.fake.emit("exhibits.changed", { exhibitId: "newer", revision: 0, deleted: true });
    await settle();
    expect(frame(harness.root)).toBeNull();
    expect(harness.root.querySelector(".exhibits-viewer")?.textContent).toContain("This Exhibit was deleted.");
    expect(listedTitles(harness.root)).toEqual(["Older"]);
  });

  it("resubscribes and catches up when the change stream ends", async () => {
    const harness = await mountExhibits();
    await select(harness, "Newer");
    harness.revisions.set("newer", 5);
    harness.listing = { exhibits: [{ ...NEWER, revision: 5 }, OLDER] };
    harness.fake.terminate("exhibits.changed", createPackageRequestError({ code: "subscription_lagged" }));
    await new Promise((resolve) => setTimeout(resolve, 50));
    await settle();
    expect(calls(harness.fake, "exhibits.changed")).toHaveLength(2);
    expect(calls(harness.fake, "exhibit.list").length).toBeGreaterThanOrEqual(2);
    expect(frame(harness.root)?.getAttribute("src")).toBe("teamarness-content://localhost/newer-r5/index.html");
    // The new stream is live.
    harness.revisions.set("newer", 6);
    await harness.fake.emit("exhibits.changed", { exhibitId: "newer", revision: 6, deleted: false });
    await settle();
    expect(frame(harness.root)?.getAttribute("src")).toBe("teamarness-content://localhost/newer-r6/index.html");
  });

  it("ignores messages from the Exhibit window", async () => {
    const harness = await mountExhibits();
    await select(harness, "Newer");
    const view = frame(harness.root);
    const before = harness.root.innerHTML;
    const callCount = harness.fake.calls.length;
    const source = view?.contentWindow ?? null;
    for (const data of [
      { kind: "teamarness.bridge.connect", protocol: 2 },
      { kind: "request", id: 1, operation: "exhibit.open", payload: { exhibitId: "older" } },
      { exhibitId: "older", revision: 9, deleted: false },
      "select:older",
    ]) {
      window.dispatchEvent(new MessageEvent("message", { data, source, origin: "null" }));
    }
    await settle();
    expect(harness.root.innerHTML).toBe(before);
    expect(harness.fake.calls.length).toBe(callCount);
    expect(view?.getAttribute("src")).toBe("teamarness-content://localhost/newer-r2/index.html");
  });

  it("unmounts cleanly and stops following changes", async () => {
    const harness = await mountExhibits();
    await select(harness, "Newer");
    await harness.unmount();
    expect(harness.root.childElementCount).toBe(0);
    const callCount = harness.fake.calls.length;
    await harness.fake.emit("exhibits.changed", { exhibitId: "newer", revision: 3, deleted: false });
    await settle();
    expect(harness.fake.calls.length).toBe(callCount);
  });
});
