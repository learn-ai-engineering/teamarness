// @vitest-environment happy-dom
import { createFakePackageClient, createPackageRequestError, createTestMountContext } from "@teamarness/package-ui-sdk/testing";
import type { PackageValue } from "@teamarness/package-ui-sdk";
import { describe, expect, it } from "vitest";
import type { DocumentsContract } from "./contract.generated";
import { mount } from "./index";

type Listing = DocumentsContract["requests"]["documents.mapping.list"]["response"];
type Report = NonNullable<DocumentsContract["requests"]["documents.mapping.set"]["response"]["report"]>;

const planning = "1".repeat(64);
const empty = "2".repeat(64);
const created = "3".repeat(64);

function listing(): Listing {
  return {
    collections: [
      {
        collectionId: planning, name: "Planning", archived: false,
        mapping: {
          path: "/Users/me/plans", state: "active", recursive: false, pendingFolderChanges: 1,
          counts: { files: 3, refused: 2, subfolders: 1 },
          refused: [
            { name: "Links.md", reason: "lossy", detail: "example com" },
            { name: "Huge.md", reason: "too_large" },
          ],
          subfolders: ["archive"], gitRoot: null,
        },
      },
      { collectionId: empty, name: "Empty", archived: false, mapping: null },
      { collectionId: "4".repeat(64), name: "Old", archived: true, mapping: null },
    ],
  };
}

function report(gitRoot: string | null = null): Report {
  return {
    counts: { files: 2, refused: 1, reformatted: 1, subfolders: 0, relocations: 1 },
    blocked: false,
    relocations: [{ documentId: created, from: "Root.md", to: "Moved/Root.md" }],
    files: [{ name: "Plan.md", bytes: 14 }, { name: "Wrapped.md", bytes: 12 }],
    refused: [{ name: "Lossy.md", reason: "lossy", detail: "https example com" }],
    reformatted: ["Wrapped.md"],
    subfolders: [],
    gitRoot,
  };
}

function tick(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

/** Polls until `done` holds, for at most two seconds. */
async function settle(done: () => boolean): Promise<void> {
  const deadline = Date.now() + 2_000;
  while (!done() && Date.now() < deadline) await tick();
  expect(done()).toBe(true);
}

function texts(root: ParentNode, selector: string): string[] {
  return [...root.querySelectorAll(selector)].map((found) => found.textContent ?? "");
}

type Handlers = {
  list?: () => Listing;
  set?: (payload: PackageValue) => unknown;
  enable?: (payload: PackageValue) => unknown;
  draft?: PackageValue;
};

function backend(handlers: Handlers = {}) {
  let current = handlers.list ?? listing;
  const drafts: PackageValue[] = [];
  const fake = createFakePackageClient({
    requests: {
      "documents.mapping.list": () => current(),
      "documents.mapping.enable-recursive": (payload) => (handlers.enable?.(payload) ?? {}) as PackageValue,
      "documents.mapping.set": (payload) => (handlers.set?.(payload) ?? {}) as PackageValue,
      "documents.mapping.clear": (payload) => ({ collectionId: (payload as { collectionId: string }).collectionId, cleared: true }),
      "documents.mapping.draft.get": () => handlers.draft ?? {},
      "documents.mapping.draft.set": (payload) => {
        drafts.push(payload);
        return {};
      },
    },
    subscriptions: { "documents.changes": () => undefined },
  });
  return { fake, drafts, setList: (next: () => Listing) => { current = next; } };
}

async function mounted(fake: ReturnType<typeof createFakePackageClient>) {
  const root = document.createElement("div").attachShadow({ mode: "open" });
  const unmount = await mount(createTestMountContext({ client: fake.client, root, displayName: "Documents", view: "settings" }));
  return { root, unmount };
}

function input(root: ShadowRoot, label: string): HTMLInputElement {
  return root.querySelector(`[aria-label="${label}"]`) as HTMLInputElement;
}

function type(field: HTMLInputElement | HTMLSelectElement, value: string): void {
  field.value = value;
  field.dispatchEvent(new Event(field instanceof HTMLSelectElement ? "change" : "input"));
}

function buttonNamed(root: ShadowRoot, text: string): HTMLButtonElement {
  const found = [...root.querySelectorAll("button")].find((candidate) => candidate.textContent === text);
  if (!found) throw new Error(`no button ${text}`);
  return found;
}

function calls(fake: ReturnType<typeof createFakePackageClient>, operation: string): PackageValue[] {
  return fake.calls.filter((call) => call.operation === operation).map((call) => call.payload);
}

describe("Documents settings view", () => {
  it("previews subfolders, cancels without enabling, then confirms sharing without losing drafts", async () => {
    let recursive = false;
    const { fake } = backend({
      list: () => ({ collections: listing().collections.map((c) => c.mapping ? { ...c, mapping: { ...c.mapping, recursive, pendingFolderChanges: recursive ? 0 : 1 } } : c) }),
      enable: (payload) => {
        if ((payload as { dryRun?: boolean }).dryRun) return { collectionId: planning, dryRun: true, recursive: false, changed: false, report: report() };
        recursive = true;
        return { collectionId: planning, dryRun: false, recursive: true, changed: true };
      },
    });
    const { root, unmount } = await mounted(fake);
    type(input(root, "Folder path"), "/draft/path");
    type(input(root, "New Collection name"), "Draft");
    expect(root.textContent).toContain("Subfolders not synced");
    expect(root.textContent).toContain("1 folder change waiting for confirmation");
    buttonNamed(root, "Enable subfolders").click();
    await settle(() => root.querySelector(".mapping-recursive-preview h3") !== null);
    expect(calls(fake, "documents.mapping.enable-recursive")).toEqual([{ collectionId: planning, dryRun: true }]);
    expect(root.querySelector(".mapping-recursive-preview")?.textContent).toContain("shared with the Workspace");
    expect(root.querySelector(".mapping-recursive-preview")?.textContent).toContain("Root.md → Moved/Root.md");
    buttonNamed(root, "Cancel").click();
    expect(calls(fake, "documents.mapping.enable-recursive")).toHaveLength(1);
    expect(root.querySelector(".mapping-recursive-preview h3")).toBeNull();
    buttonNamed(root, "Enable subfolders").click();
    await settle(() => root.querySelector(".mapping-recursive-preview h3") !== null);
    buttonNamed(root, "Confirm enable subfolders").click();
    await settle(() => root.textContent?.includes("Subfolders synced") === true);
    expect(calls(fake, "documents.mapping.enable-recursive").at(-1)).toEqual({ collectionId: planning, dryRun: false, confirmGit: false });
    expect(root.querySelector(".mapping-collection")?.textContent).not.toContain("not synced");
    expect(input(root, "Folder path").value).toBe("/draft/path");
    expect(input(root, "New Collection name").value).toBe("Draft");
    await unmount();
  });

  it("blocks unsafe relocation confirmation and requires the git warning for safe activation", async () => {
    let blocked = true;
    const { fake } = backend({ enable: (payload) => {
      if ((payload as { dryRun?: boolean }).dryRun) return { collectionId: planning, dryRun: true, recursive: false, changed: false, report: { ...report("/repo"), blocked } };
      throw createPackageRequestError({ packageError: { code: "documents.mapping_refused", details: { reason: "relocation_blocked" } } });
    } });
    const { root, unmount } = await mounted(fake);
    buttonNamed(root, "Enable subfolders").click();
    await settle(() => root.querySelector(".mapping-recursive-preview h3") !== null);
    expect(buttonNamed(root, "Confirm enable subfolders").disabled).toBe(true);
    buttonNamed(root, "Cancel").click();
    blocked = false;
    buttonNamed(root, "Enable subfolders").click();
    await settle(() => root.querySelector(".mapping-recursive-preview h3") !== null);
    expect(buttonNamed(root, "Confirm enable subfolders").disabled).toBe(true);
    const git = input(root, "Enable subfolders inside a git working tree");
    git.checked = true;
    git.dispatchEvent(new Event("change"));
    expect(buttonNamed(root, "Confirm enable subfolders").disabled).toBe(false);
    buttonNamed(root, "Confirm enable subfolders").click();
    await settle(() => root.textContent?.includes("A required file relocation is blocked") === true);
    expect(calls(fake, "documents.mapping.enable-recursive").at(-1)).toEqual({ collectionId: planning, dryRun: false, confirmGit: true });
    await unmount();
  });

  it("shows a filesystem folder refusal without implying the logical folder was changed", async () => {
    const original = listing().collections[0]!;
    const current: Listing = { collections: [{
      ...original,
      mapping: {
        ...original.mapping!,
        refused: [{ name: ".hidden/Note.md", reason: "invalid_folder", detail: "Logical folder is valid; hidden disk paths are excluded" }],
        counts: { ...original.mapping!.counts, refused: 1 },
      },
    }] };
    const { fake } = backend({ list: () => current });
    const { root, unmount } = await mounted(fake);
    expect(root.textContent).toContain(".hidden/Note.md");
    expect(root.textContent).toContain("This folder path cannot be synced to this filesystem");
    expect(root.textContent).toContain("Logical folder is valid; hidden disk paths are excluded");
    unmount();
  });
  it("lists Collections with their mapping, counts and an expandable refusal list", async () => {
    const { fake } = backend();
    const { root, unmount } = await mounted(fake);
    // The Shell's page header and Configuration tab name the view: no heading of its own.
    expect(root.querySelector("h1")).toBeNull();
    expect(root.querySelector(".settings-description")?.textContent).toContain("keeps the files and the Documents in sync.");
    expect(texts(root, ".settings-group h2")).toEqual(["Collections", "Map a folder"]);
    // No panels: the groups are not kit sections.
    expect(root.querySelector(".teamarness-section")).toBeNull();
    expect(root.querySelector(".collection-rail")).toBeNull();
    expect(texts(root, ".mapping-collection-name")).toEqual(["Planning", "Empty"]);
    expect(texts(root, ".mapping-state")).toEqual(["Synced", "Not mapped"]);
    expect(root.querySelector(".mapping-path")?.textContent).toBe("/Users/me/plans");
    expect(root.querySelector(".mapping-counts")?.textContent).toBe("3 files · 2 refused · 1 subfolder not synced yet");
    // Unmap is an icon button in the Collection's header row, named for the Collection.
    const unmap = root.querySelector('.mapping-collection-header [aria-label="Unmap Planning"]') as HTMLButtonElement;
    expect(unmap.title).toBe("Unmap");
    expect(unmap.querySelector("svg")).not.toBeNull();
    expect(unmap.querySelector("[role=tooltip]")?.textContent).toBe("Unmap");
    expect(root.querySelectorAll(".mapping-unmap")).toHaveLength(1);
    const details = root.querySelector(".mapping-refused") as HTMLDetailsElement;
    expect(details.open).toBe(false);
    expect(details.querySelector("summary")?.textContent).toBe("2 refused files");
    expect(texts(details, "li")).toEqual([
      "Links.md: Importing it would lose text (example com)",
      "Huge.md: Larger than a Document can be",
    ]);
    // The Collection selector offers only unarchived, unmapped Collections.
    expect([...(input(root, "Collection") as unknown as HTMLSelectElement).options].map((option) => option.text))
      .toEqual(["New Collection from this folder", "Empty"]);
    await unmount();
    expect(root.childElementCount).toBe(0);
  });

  it("refreshes the list on every documents.changes event and keeps a refusal list open", async () => {
    const { fake, setList } = backend();
    const { root, unmount } = await mounted(fake);
    const details = root.querySelector(".mapping-refused") as HTMLDetailsElement;
    details.open = true;
    details.dispatchEvent(new Event("toggle"));
    setList(() => {
      const next = listing();
      const [first] = next.collections;
      return { collections: [{ ...first!, mapping: { ...first!.mapping!, state: "unavailable" } }, ...next.collections.slice(1)] };
    });
    const before = calls(fake, "documents.mapping.list").length;
    await fake.emit("documents.changes", { collections: [], documents: [], availability: "current", warnings: [] });
    await settle(() => calls(fake, "documents.mapping.list").length > before
      && root.querySelector(".mapping-state")?.textContent === "Folder unavailable");
    expect((root.querySelector(".mapping-refused") as HTMLDetailsElement).open).toBe(true);
    await unmount();
  });

  it("previews a folder, then creates a Collection from it", async () => {
    const { fake } = backend({
      set: (payload) => (payload as { dryRun?: boolean }).dryRun
        ? { collectionId: null, path: "/Users/me/notes", direction: "import", dryRun: true, report: report() }
        : { collectionId: created, path: "/Users/me/notes", direction: "import", dryRun: false },
    });
    const { root, unmount } = await mounted(fake);
    const preview = buttonNamed(root, "Preview");
    const map = buttonNamed(root, "Create Collection from folder");
    const hint = root.querySelector(".mapping-hint") as HTMLElement;
    expect(preview.disabled).toBe(true);
    expect(map.disabled).toBe(true);
    expect(hint.textContent).toBe("Enter a folder path and Preview it to enable Create Collection from folder.");
    type(input(root, "Folder path"), " /Users/me/notes ");
    expect(preview.disabled).toBe(false);
    // Until a preview exists, Preview is the primary action and Create waits, saying why.
    expect(preview.classList.contains("teamarness-control--primary")).toBe(true);
    expect(map.classList.contains("teamarness-control--secondary")).toBe(true);
    expect(map.disabled).toBe(true);
    expect(map.getAttribute("aria-describedby")).toBe(hint.id);
    expect(hint.textContent).toBe("Preview the folder to enable Create Collection from folder.");
    preview.click();
    await settle(() => root.querySelector(".mapping-summary") !== null);
    expect(preview.classList.contains("teamarness-control--secondary")).toBe(true);
    expect(map.classList.contains("teamarness-control--primary")).toBe(true);
    expect(calls(fake, "documents.mapping.set")).toEqual([{ newCollectionName: "Untitled", path: "/Users/me/notes", dryRun: true }]);
    expect(root.querySelector(".mapping-summary")?.textContent).toBe("2 files will be imported.");
    expect(texts(root, ".mapping-files li")).toEqual(["Plan.md", "Wrapped.md"]);
    expect(texts(root, ".mapping-report .mapping-refused-list li")).toEqual(["Lossy.md: Importing it would lose text (https example com)"]);
    expect(texts(root, ".mapping-reformatted li")).toEqual(["Wrapped.md"]);
    expect(texts(root, ".mapping-subfolders li")).toEqual([]);
    expect((root.querySelector(".mapping-git-confirm") as HTMLElement).hidden).toBe(true);
    // A name is needed to create the Collection.
    expect(map.disabled).toBe(true);
    expect(hint.textContent).toBe("Name the new Collection to enable Create Collection from folder.");
    type(input(root, "New Collection name"), "Notes");
    expect(map.disabled).toBe(false);
    expect(hint.hidden).toBe(true);
    map.click();
    await settle(() => calls(fake, "documents.mapping.set").length === 2 && root.querySelector("[role=status]")?.textContent !== "");
    expect(calls(fake, "documents.mapping.set")[1]).toEqual({ newCollectionName: "Notes", path: "/Users/me/notes", dryRun: false, confirmGit: false });
    expect(root.querySelector("[role=status]")?.textContent).toBe("Notes is mapped to /Users/me/notes.");
    expect(input(root, "Folder path").value).toBe("");
    expect(root.querySelector(".mapping-summary")).toBeNull();
    await unmount();
  });

  it("needs a new preview after the path changes, and shows why a folder is refused", async () => {
    const { fake } = backend({
      set: (payload) => {
        if ((payload as { path: string }).path === "/Users/me/full") {
          throw createPackageRequestError({ packageError: { code: "documents.mapping_refused", details: { reason: "not_empty" } } });
        }
        return { collectionId: empty, path: "/Users/me/empty", direction: "export", dryRun: true, report: { ...report(), counts: { files: 0, refused: 0, reformatted: 0, subfolders: 0 }, files: [], refused: [], reformatted: [], subfolders: [] } };
      },
    });
    const { root, unmount } = await mounted(fake);
    type(input(root, "Collection") as unknown as HTMLSelectElement, empty);
    expect((root.querySelector(".mapping-form label:nth-of-type(3)") as HTMLElement).hidden).toBe(true);
    const map = buttonNamed(root, "Map folder");
    type(input(root, "Folder path"), "/Users/me/empty");
    buttonNamed(root, "Preview").click();
    await settle(() => root.querySelector(".mapping-summary") !== null);
    expect(root.querySelector(".mapping-summary")?.textContent).toBe("The folder has no Markdown files. Every Document of the Collection will be written to it.");
    expect(calls(fake, "documents.mapping.set")[0]).toEqual({ collectionId: empty, path: "/Users/me/empty", dryRun: true });
    expect(map.disabled).toBe(false);
    type(input(root, "Folder path"), "/Users/me/full");
    expect(root.querySelector(".mapping-summary")).toBeNull();
    expect(map.disabled).toBe(true);
    buttonNamed(root, "Preview").click();
    await settle(() => root.querySelector("[role=alert]")?.textContent !== "");
    expect(root.querySelector("[role=alert]")?.textContent).toBe(
      "The folder and the Collection both have content. Map an empty folder, or create a new Collection from this folder.");
    await unmount();
  });

  it("maps a folder inside a git working tree only after an explicit confirmation", async () => {
    const { fake } = backend({
      set: (payload) => (payload as { dryRun?: boolean }).dryRun
        ? { collectionId: empty, path: "/Users/me/repo/notes", direction: "export", dryRun: true, report: report("/Users/me/repo") }
        : { collectionId: empty, path: "/Users/me/repo/notes", direction: "export", dryRun: false },
    });
    const { root, unmount } = await mounted(fake);
    type(input(root, "Collection") as unknown as HTMLSelectElement, empty);
    type(input(root, "Folder path"), "/Users/me/repo/notes");
    buttonNamed(root, "Preview").click();
    await settle(() => root.querySelector(".mapping-summary") !== null);
    const warning = root.querySelector(".mapping-git-confirm") as HTMLElement;
    expect(warning.hidden).toBe(false);
    expect(warning.textContent).toContain("inside the git working tree /Users/me/repo");
    const map = buttonNamed(root, "Map folder");
    expect(map.disabled).toBe(true);
    expect(root.querySelector(".mapping-hint")?.textContent).toBe("Confirm the git warning below to enable Map folder.");
    const confirm = input(root, "Map a folder inside a git working tree");
    confirm.checked = true;
    confirm.dispatchEvent(new Event("change"));
    expect(map.disabled).toBe(false);
    map.click();
    await settle(() => calls(fake, "documents.mapping.set").length === 2);
    expect(calls(fake, "documents.mapping.set")[1]).toEqual({ collectionId: empty, path: "/Users/me/repo/notes", dryRun: false, confirmGit: true });
    await unmount();
  });

  it("unmaps a Collection and says the files stay", async () => {
    const { fake, setList } = backend();
    const { root, unmount } = await mounted(fake);
    setList(() => ({ collections: listing().collections.map((collection) => ({ ...collection, mapping: null })) }));
    (root.querySelector('[aria-label="Unmap Planning"]') as HTMLButtonElement).click();
    await settle(() => root.querySelector('[aria-label="Unmap Planning"]') === null);
    expect(calls(fake, "documents.mapping.clear")).toEqual([{ collectionId: planning }]);
    expect(root.querySelector("[role=status]")?.textContent).toBe("Planning is no longer mapped. Its files stay in the folder.");
    expect(texts(root, ".mapping-state")).toEqual(["Not mapped", "Not mapped"]);
    await unmount();
  });

  it("remembers the unsent path, name and Collection across a Package switch", async () => {
    const first = backend();
    const { root, unmount } = await mounted(first.fake);
    type(input(root, "Folder path"), "/Users/me/notes");
    type(input(root, "New Collection name"), "Notes");
    await settle(() => first.drafts.length > 0);
    expect(first.drafts.at(-1)).toEqual({ path: "/Users/me/notes", name: "Notes" });
    type(input(root, "Collection") as unknown as HTMLSelectElement, empty);
    // Unmounting saves a draft that is still waiting.
    await unmount();
    expect(first.drafts.at(-1)).toEqual({ path: "/Users/me/notes", name: "Notes", collectionId: empty });

    const second = backend({ draft: first.drafts.at(-1)! });
    const again = await mounted(second.fake);
    expect(input(again.root, "Folder path").value).toBe("/Users/me/notes");
    expect(input(again.root, "New Collection name").value).toBe("Notes");
    expect((input(again.root, "Collection") as unknown as HTMLSelectElement).value).toBe(empty);
    await again.unmount();
  });
});
