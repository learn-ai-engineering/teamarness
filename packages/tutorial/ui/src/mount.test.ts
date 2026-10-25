import type { PackageClient, PackageMountContext, PackageValue } from "@teamarness/package-ui-sdk";
import { describe, expect, it } from "vitest";

import { mount } from "./index";

type Listener = () => void;

class FakeCSSStyleSheet {
  cssText = "";

  replaceSync(css: string): void {
    this.cssText = css;
  }
}

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
  adoptedStyleSheets: CSSStyleSheet[] = [];

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
globalThis.CSSStyleSheet = FakeCSSStyleSheet as unknown as typeof CSSStyleSheet;

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

  it("renders a page header and sections from the Package UI kit", async () => {
    const root = new FakeElement("shadow-root");
    await mount(context(root));

    const [title] = descendants(root, (element) => element.tagName === "h1");
    expect(title?.className).toBe("teamarness-page-title");
    expect(title?.textContent).toBe("Tutorial");
    expect(descendants(root, (element) => element.tagName === "section" && element.className.includes("teamarness-section"))).toHaveLength(2);
    const [savedTitle] = descendants(root, (element) => element.tagName === "h2");
    expect(savedTitle?.className).toBe("teamarness-section-title");
    expect(savedTitle?.textContent).toBe("Saved art");
  });

  it("adopts and removes only its constructed stylesheets without creating inline styles", async () => {
    const root = new FakeElement("shadow-root");
    const sheetBefore = new FakeCSSStyleSheet() as unknown as CSSStyleSheet;
    const sheetAfter = new FakeCSSStyleSheet() as unknown as CSSStyleSheet;
    root.adoptedStyleSheets = [sheetBefore];

    const unmount = await mount(context(root));

    expect(root.adoptedStyleSheets).toHaveLength(3);
    expect(root.adoptedStyleSheets[0]).toBe(sheetBefore);
    const kitSheet = root.adoptedStyleSheets[1] as unknown as FakeCSSStyleSheet;
    expect(kitSheet.cssText).toContain(".teamarness-page-title");
    const tutorialSheet = root.adoptedStyleSheets[2] as unknown as FakeCSSStyleSheet;
    expect(tutorialSheet.cssText).toMatch(/\.art\s*\{[^}]*grid-template-columns/);
    expect(descendants(root, (element) => element.tagName === "style")).toHaveLength(0);

    root.adoptedStyleSheets = [...root.adoptedStyleSheets, sheetAfter];
    await unmount();
    await unmount();
    expect(root.adoptedStyleSheets).toEqual([sheetBefore, sheetAfter]);
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
    view: "main",
    uiHostVersion: 2,
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
