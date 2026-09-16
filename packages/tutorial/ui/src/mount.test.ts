import type { PackageClient, PackageMountContext, PackageValue } from "@teamarness/package-ui-sdk";
import { describe, expect, it } from "vitest";

import { mount } from "./index";

type Listener = () => void;

class FakeElement {
  readonly children: FakeElement[] = [];
  readonly attributes = new Map<string, string>();
  readonly listeners = new Map<string, Listener[]>();
  parent: FakeElement | undefined;
  className = "";
  textContent = "";
  value = "";
  type = "";
  disabled = false;

  constructor(readonly tagName: string) {}

  get options(): FakeElement[] {
    return this.children;
  }

  append(...nodes: Array<FakeElement | string>): void {
    for (const node of nodes) {
      if (typeof node === "string") {
        this.textContent += node;
        continue;
      }
      node.parent = this;
      this.children.push(node);
    }
  }

  replaceChildren(...nodes: FakeElement[]): void {
    for (const child of this.children) child.parent = undefined;
    this.children.length = 0;
    this.append(...nodes);
  }

  remove(): void {
    if (!this.parent) return;
    const index = this.parent.children.indexOf(this);
    if (index >= 0) this.parent.children.splice(index, 1);
    this.parent = undefined;
  }

  setAttribute(name: string, value: string): void {
    this.attributes.set(name, value);
  }

  getAttribute(name: string): string | null {
    return this.attributes.get(name) ?? null;
  }

  addEventListener(name: string, listener: Listener): void {
    this.listeners.set(name, [...(this.listeners.get(name) ?? []), listener]);
  }

  removeEventListener(name: string, listener: Listener): void {
    this.listeners.set(name, (this.listeners.get(name) ?? []).filter((entry) => entry !== listener));
  }
}

class FakeDocument {
  createElement(tagName: string): FakeElement {
    return new FakeElement(tagName);
  }
}

globalThis.document = new FakeDocument() as unknown as Document;

describe("Tutorial UI mount", () => {
  it("places the unlabeled Palette and Pattern selects in phrase order", async () => {
    const root = new FakeElement("shadow-root");
    await mount(context(root));

    const selects = descendants(root, (element) => element.tagName === "select");
    expect(selects.map((select) => select.getAttribute("aria-label"))).toEqual([
      "Palette",
      "Pattern",
    ]);
    expect(descendants(root, (element) => element.tagName === "label")).toHaveLength(0);

    const [artwork] = descendants(root, (element) => element.className === "art");
    expect(artwork?.getAttribute("role")).toBe("img");
    expect(artwork?.children).toHaveLength(192);
    expect(artwork?.children.every((cell) => cell.getAttribute("aria-hidden") === "true")).toBe(true);
  });
});

function context(root: FakeElement): PackageMountContext {
  const client: PackageClient = {
    request: async <TResponse extends PackageValue>(operation: string): Promise<TResponse> => {
      if (operation === "tutorial.read") {
        return {
          algorithms: [
            { id: "waves", label: "Waves" },
            { id: "spiral", label: "Spiral" },
          ],
          palettes: [
            { id: "garden", label: "Garden", values: ["🌱", "🌿", "🍀", "🌸", "🌻"] },
            { id: "ocean", label: "Ocean", values: ["💧", "🐚", "🐠", "🐬", "🌊"] },
          ],
          artworks: [],
        } as unknown as TResponse;
      }
      return {
        recipe: {
          generatorVersion: 1,
          algorithm: "waves",
          palette: "garden",
          seed: 7,
          width: 16,
          height: 12,
        },
        cells: Array.from({ length: 192 }, () => "🌱"),
      } as unknown as TResponse;
    },
    subscribe: async () => ({ close: async () => undefined }),
  };
  return {
    root,
    package: { displayName: "Tutorial" },
    workspace: { id: "workspace", name: "Test" },
    uiHostVersion: 1,
    themeTokens: [],
    client,
  } as unknown as PackageMountContext;
}

function descendants(
  root: FakeElement,
  predicate: (element: FakeElement) => boolean,
): FakeElement[] {
  return root.children.flatMap((child) => [
    ...(predicate(child) ? [child] : []),
    ...descendants(child, predicate),
  ]);
}
