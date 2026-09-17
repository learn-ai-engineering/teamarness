import { afterEach, describe, expect, it, vi } from "vitest";

import type { PackageMountContext, PackageSubscription } from "@teamarness/package-ui-sdk";
import { TEAMARNESS_THEME_TOKENS } from "@teamarness/package-ui-sdk";
import { mount } from "./index";

class FakeCSSStyleSheet {
  cssText = "";

  replaceSync(css: string): void {
    this.cssText = css;
  }
}

type FakeElement = HTMLElement & {
  children: FakeElement[];
  removed: boolean;
};

const createdElements: FakeElement[] = [];

function fakeElement(tagName: string): FakeElement {
  const attributes = new Map<string, string>();
  const element = {
    children: [] as FakeElement[],
    className: "",
    tagName: tagName.toUpperCase(),
    textContent: "",
    removed: false,
    append(this: FakeElement, ...children: FakeElement[]) { this.children.push(...children); },
    remove(this: FakeElement) { this.removed = true; },
    setAttribute(name: string, value: string) { attributes.set(name, value); },
    getAttribute(name: string) { return attributes.get(name) ?? null; },
    addEventListener() {},
    removeEventListener() {},
  } as unknown as FakeElement;
  createdElements.push(element);
  return element;
}

function computedStyle(element: Element): CSSStyleDeclaration {
  const className = (element as HTMLElement).className;
  return {
    display: className.includes("teamarness-stack") ? "flex" : "block",
    flexDirection: className.includes("teamarness-stack") ? "column" : "row",
    borderTopStyle: className.includes("teamarness-control") ? "solid" : "none",
    backgroundColor: className.includes("teamarness-control--primary") ? "rgb(11, 114, 93)" : "rgba(0, 0, 0, 0)",
    borderTopColor: className.includes("teamarness-control--primary") ? "rgb(11, 114, 93)" : "rgb(0, 0, 0)",
    getPropertyValue: (name: string) => name === "--teamarness-color-accent" ? "#0b725d" : "",
  } as unknown as CSSStyleDeclaration;
}

function mountContext(root: ShadowRoot): PackageMountContext {
  const subscription: PackageSubscription = { close: vi.fn(async () => undefined) };
  return {
    root,
    package: {
      identity: "dev.teamarness.reference",
      version: "1.0.0",
      displayName: "Reference Package",
      source: "bundled",
      trust: "application_bundled",
      lifecycle: "active",
      processGeneration: 1,
    },
    workspace: { id: "workspace", name: "Workspace" },
    uiHostVersion: 1,
    themeTokens: TEAMARNESS_THEME_TOKENS,
    client: {
      request: async () => ({
        counter: 0,
        backgroundMarker: 0,
        membershipVersion: "1",
        signingVerified: true,
        processGeneration: "1",
        packageVersion: "1.0.0",
      }),
      subscribe: async () => subscription,
    },
  } as PackageMountContext;
}

afterEach(() => {
  createdElements.length = 0;
  vi.unstubAllGlobals();
});

describe("reference Package UI", () => {
  it("uses only its mount-scoped client for Package operations", () => {
    const source = mount.toString();
    expect(source).toContain('client.request("reference.read"');
    expect(source).toContain('client.request("reference.increment"');
    expect(source).toContain('client.subscribe("reference.changes"');
    expect(source).not.toContain("invoke(");
    expect(source).not.toContain("document.head");
  });

  it("adopts Package-owned and kit sheets, reports computed-style readiness, and cleans up only those sheets", async () => {
    vi.stubGlobal("CSSStyleSheet", FakeCSSStyleSheet);
    vi.stubGlobal("document", { createElement: (tagName: string) => fakeElement(tagName) });
    vi.stubGlobal("getComputedStyle", vi.fn(computedStyle));
    const sheetBefore = new FakeCSSStyleSheet() as unknown as CSSStyleSheet;
    const sheetAfter = new FakeCSSStyleSheet() as unknown as CSSStyleSheet;
    const root = {
      adoptedStyleSheets: [sheetBefore],
      append: vi.fn(),
    } as unknown as ShadowRoot;

    const unmount = await mount(mountContext(root));

    expect(root.adoptedStyleSheets).toHaveLength(3);
    expect(root.adoptedStyleSheets[0]).toBe(sheetBefore);
    const packageSheet = root.adoptedStyleSheets[2] as unknown as FakeCSSStyleSheet;
    expect(packageSheet.cssText).toMatch(/\.reference-action\s*\{[^}]*align-self:\s*flex-start/);
    const heading = createdElements.find((element) => element.textContent === "Reference Package");
    expect(heading?.tagName).toBe("H1");
    expect(heading?.className).toContain("teamarness-page-title");
    const action = createdElements.find((element) => element.textContent === "Increment counter");
    expect(action?.className).toContain("reference-action");
    expect(createdElements.every((element) => element.tagName !== "STYLE")).toBe(true);
    const kitLabelIndex = createdElements.findIndex((element) => element.textContent === "UI kit");
    expect(kitLabelIndex).toBeGreaterThanOrEqual(0);
    expect(createdElements[kitLabelIndex + 1]?.textContent).toBe("Ready");
    expect(createdElements[kitLabelIndex + 1]?.getAttribute?.("aria-label")).toBe("UI kit Ready");
    expect(getComputedStyle).toHaveBeenCalled();

    root.adoptedStyleSheets = [...root.adoptedStyleSheets, sheetAfter];
    await unmount();
    await unmount();
    expect(root.adoptedStyleSheets).toEqual([sheetBefore, sheetAfter]);
  });

  it("adopts the shared theme contract and optional UI kit", () => {
    const source = mount.toString();
    expect(source).toContain("TEAMARNESS_THEME_TOKENS");
    expect(source).toContain("installPackageUiKit");
    expect(source).toContain("teamarness-control");
    expect(source).toContain("teamarness-page");
    expect(source).toContain("teamarness-section");
  });

  it("keeps qualification state while rendering only display-safe readiness labels", () => {
    const source = mount.toString();
    for (const label of ["Package UI", "Workspace", "Theme contract", "UI kit", "Membership access", "Signing round trip"]) {
      expect(source).toContain(label);
    }
    for (const diagnostic of [
      "package.identity",
      "workspace.id",
      "package.source",
      "package.trust",
      "package.lifecycle",
      "package.processGeneration",
      "membershipVersion",
      "processGeneration",
    ]) {
      expect(source).not.toContain(diagnostic);
    }
  });
});
