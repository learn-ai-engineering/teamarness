// @vitest-environment happy-dom

import { createFakePackageClient, createTestMountContext } from "@teamarness/package-ui-sdk/testing";
import type { PackageValue } from "@teamarness/package-ui-sdk";
import { describe, expect, it } from "vitest";
import type { DocumentsSnapshot } from "./index";
import { mount } from "./index";

const planning = "1".repeat(64);
const archive = "2".repeat(64);
const doc = "3".repeat(64);

function snapshot(): DocumentsSnapshot {
  return {
    availability: "current",
    warnings: [],
    collections: [
      { id: planning, name: "Planning", archived: false },
      { id: archive, name: "Old", archived: true },
    ],
    documents: [{ id: doc, collectionId: planning, title: "Q4 plan", archived: false }],
  };
}

function tick(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

function backend(current: () => DocumentsSnapshot) {
  const calls: { operation: string; payload: PackageValue }[] = [];
  const record = (operation: string) => (payload: PackageValue) => {
    calls.push({ operation, payload });
    return { ok: true as const, id: "9".repeat(64) };
  };
  const fake = createFakePackageClient({
    requests: {
      "documents.snapshot": () => current(),
      "documents.collection.create": record("documents.collection.create"),
      "documents.collection.rename": record("documents.collection.rename"),
      "documents.collection.archive": record("documents.collection.archive"),
      "documents.document.create": record("documents.document.create"),
      "documents.document.rename": record("documents.document.rename"),
      "documents.document.archive": record("documents.document.archive"),
      "documents.document.move": record("documents.document.move"),
    },
    subscriptions: { "documents.changes": () => undefined },
  });
  return { fake, calls };
}

function texts(root: ParentNode, selector: string): string[] {
  return [...root.querySelectorAll(selector)].map((element) => element.textContent ?? "");
}

describe("Documents mount", () => {
  it("lists Collections and Documents, hides archived until asked, and unmounts cleanly", async () => {
    const { fake } = backend(snapshot);
    const root = document.createElement("div").attachShadow({ mode: "open" });
    const unmount = await mount(createTestMountContext({ client: fake.client, root, displayName: "Documents" }));
    expect(root.querySelector("h1")?.textContent).toBe("Documents");
    expect(root.querySelector("[role=status]")?.textContent).toBe("Up to date");
    expect(texts(root, ".collection-button")).toEqual(["Planning"]);
    expect(root.querySelector(".collection-button")?.getAttribute("aria-current")).toBe("page");
    expect(texts(root, ".document-title")).toEqual(["Q4 plan"]);
    const toggle = root.querySelector(".archived-toggle input") as HTMLInputElement;
    toggle.checked = true;
    toggle.dispatchEvent(new Event("change"));
    expect(texts(root, ".collection-button")).toEqual(["Planning", "Old"]);
    await unmount();
    expect(root.childElementCount).toBe(0);
  });

  it("sends create, rename, archive, and move requests with the selected Collection", async () => {
    const { fake, calls } = backend(snapshot);
    const root = document.createElement("div").attachShadow({ mode: "open" });
    const unmount = await mount(createTestMountContext({ client: fake.client, root }));

    const collectionInput = root.querySelector(".collection-create input") as HTMLInputElement;
    collectionInput.value = "  Roadmap ";
    (root.querySelector(".collection-create") as HTMLFormElement).requestSubmit();
    await tick();
    expect(calls.at(-1)).toEqual({ operation: "documents.collection.create", payload: { name: "Roadmap" } });
    expect(collectionInput.value).toBe("");

    const documentInput = root.querySelector(".document-create input") as HTMLInputElement;
    documentInput.value = "Hiring plan";
    (root.querySelector(".document-create") as HTMLFormElement).requestSubmit();
    await tick();
    expect(calls.at(-1)).toEqual({
      operation: "documents.document.create",
      payload: { collectionId: planning, title: "Hiring plan" },
    });

    const renameButton = [...root.querySelectorAll(".document-actions button")].find((b) => b.textContent === "Rename") as HTMLButtonElement;
    renameButton.click();
    const renameInput = root.querySelector(".document-rename input") as HTMLInputElement;
    expect(renameInput.value).toBe("Q4 plan");
    renameInput.value = "Q4 plan v2";
    (root.querySelector(".document-rename") as HTMLFormElement).requestSubmit();
    await tick();
    expect(calls.at(-1)).toEqual({ operation: "documents.document.rename", payload: { documentId: doc, title: "Q4 plan v2" } });
    expect(root.querySelector(".document-rename")).toBeNull();

    const archiveButton = [...root.querySelectorAll(".document-actions button")].find((b) => b.textContent === "Archive") as HTMLButtonElement;
    archiveButton.click();
    await tick();
    expect(calls.at(-1)).toEqual({ operation: "documents.document.archive", payload: { documentId: doc } });

    // Only non-archived other Collections are move destinations; "Old" is archived, so none.
    expect(root.querySelector(".document-move")).toBeNull();
    await unmount();
  });

  it("applies a pushed snapshot and keeps the selection when it survives", async () => {
    let current = snapshot();
    const { fake } = backend(() => current);
    const root = document.createElement("div").attachShadow({ mode: "open" });
    const unmount = await mount(createTestMountContext({ client: fake.client, root }));
    current = {
      ...current,
      collections: [...current.collections, { id: "4".repeat(64), name: "Research", archived: false }],
      documents: [...current.documents, { id: "5".repeat(64), collectionId: planning, title: "OKRs", archived: false }],
    };
    await fake.emit("documents.changes", current);
    expect(texts(root, ".collection-button")).toEqual(["Planning", "Research"]);
    expect(root.querySelector(".collection-button")?.getAttribute("aria-current")).toBe("page");
    expect(texts(root, ".document-title")).toEqual(["Q4 plan", "OKRs"]);
    expect(root.querySelector(".document-move")).not.toBeNull();
    // A Collection this Installation created is selected once its record is listed.
    const input = root.querySelector(".collection-create input") as HTMLInputElement;
    input.value = "Mine";
    (root.querySelector(".collection-create") as HTMLFormElement).requestSubmit();
    await tick();
    expect(root.querySelector(".collection-button[aria-current=page]")?.textContent).toBe("Planning");
    current = { ...current, collections: [...current.collections, { id: "9".repeat(64), name: "Mine", archived: false }] };
    await fake.emit("documents.changes", current);
    expect(root.querySelector(".collection-button[aria-current=page]")?.textContent).toBe("Mine");
    expect(texts(root, ".document-title")).toEqual([]);
    expect(texts(root, ".documents-empty")).toEqual(["No documents yet."]);
    await unmount();
  });

  it("surfaces a request error without losing the listing", async () => {
    const fake = createFakePackageClient({
      requests: {
        "documents.snapshot": () => snapshot(),
        "documents.collection.create": () => {
          throw Object.assign(new Error("invalid"), { kind: "teamarness.package-request-error", code: "documents.invalid_name" });
        },
      },
      subscriptions: { "documents.changes": () => undefined },
    });
    const root = document.createElement("div").attachShadow({ mode: "open" });
    const unmount = await mount(createTestMountContext({ client: fake.client, root }));
    const input = root.querySelector(".collection-create input") as HTMLInputElement;
    input.value = "x";
    (root.querySelector(".collection-create") as HTMLFormElement).requestSubmit();
    await tick();
    expect(root.querySelector("[role=alert]")?.textContent).toMatch(/Collection names/);
    expect(texts(root, ".collection-button")).toEqual(["Planning"]);
    await unmount();
  });
});
