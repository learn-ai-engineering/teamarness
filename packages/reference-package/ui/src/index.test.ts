import { afterEach, describe, expect, it, vi } from "vitest";

import type { PackageMountContext, PackageSubscription } from "@teamarness/package-ui-sdk";
import { TEAMARNESS_THEME_TOKENS } from "@teamarness/package-ui-sdk";
import { createPackageRequestError } from "@teamarness/package-ui-sdk/testing";
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
  listeners: Map<string, Set<() => void>>;
  dispatch(type: string): void;
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
    listeners: new Map<string, Set<() => void>>(),
    addEventListener(this: FakeElement, type: string, handler: () => void) {
      const existing = this.listeners.get(type) ?? new Set<() => void>();
      existing.add(handler);
      this.listeners.set(type, existing);
    },
    removeEventListener(this: FakeElement, type: string, handler: () => void) {
      this.listeners.get(type)?.delete(handler);
    },
    dispatch(this: FakeElement, type: string) {
      for (const handler of this.listeners.get(type) ?? []) handler();
    },
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
      processGeneration: "1",
    },
    workspace: { id: "workspace", name: "Workspace" },
    view: "main",
    uiHostVersion: 2,
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

describe("reference Package error fixture", () => {
  it("shows the Package error code when the Backend rejects reference.fail", async () => {
    vi.stubGlobal("CSSStyleSheet", FakeCSSStyleSheet);
    vi.stubGlobal("document", { createElement: (tagName: string) => fakeElement(tagName) });
    vi.stubGlobal("getComputedStyle", vi.fn(computedStyle));
    const root = { adoptedStyleSheets: [], append: vi.fn() } as unknown as ShadowRoot;
    const context = mountContext(root);
    const rejected = createPackageRequestError({
      stage: "ui",
      diagnosticId: "diag-1",
      packageError: { code: "reference.requested_failure", details: { reason: "requested" } },
    });
    const failing = {
      ...context,
      client: {
        ...context.client,
        request: async (operation: string) => {
          if (operation === "reference.fail") throw rejected;
          return {
            counter: 0,
            backgroundMarker: 0,
            membershipVersion: "1",
            signingVerified: true,
            processGeneration: "1",
            packageVersion: "1.0.0",
          };
        },
      },
    } as unknown as PackageMountContext;

    const unmount = await mount(failing);
    const trigger = createdElements.find((element) => element.textContent === "Trigger Package error");
    expect(trigger).toBeDefined();
    const terms = createdElements.filter((element) => element.tagName === "DT");
    const index = terms.findIndex((element) => element.textContent === "Last Package error");
    const value = createdElements.filter((element) => element.tagName === "DD")[index];
    expect(value?.textContent).toBe("None");

    trigger?.dispatch("click");
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(value?.textContent).toBe("Package error: reference.requested_failure");
    expect(value?.getAttribute("aria-label")).toBe("Package error: reference.requested_failure");
    await unmount();
  });
});

describe("reference Package settings view", () => {
  it("renders its settings view when mounted with view settings, and cleans it up", async () => {
    vi.stubGlobal("CSSStyleSheet", FakeCSSStyleSheet);
    vi.stubGlobal("document", { createElement: (tagName: string) => fakeElement(tagName) });
    vi.stubGlobal("getComputedStyle", vi.fn(computedStyle));
    const sheetBefore = new FakeCSSStyleSheet() as unknown as CSSStyleSheet;
    const root = { adoptedStyleSheets: [sheetBefore], append: vi.fn() } as unknown as ShadowRoot;
    const operations: string[] = [];
    const context = mountContext(root);
    const settings = {
      ...context,
      view: "settings",
      client: {
        ...context.client,
        request: async (operation: string) => {
          operations.push(operation);
          return context.client.request(operation, {});
        },
        subscribe: async () => { throw new Error("the settings view does not subscribe"); },
      },
    } as unknown as PackageMountContext;

    const unmount = await mount(settings);

    const heading = createdElements.find((element) => element.tagName === "H1");
    expect(heading?.textContent).toBe("Reference Package settings");
    expect(createdElements.some((element) => element.textContent === "Increment counter")).toBe(false);
    const terms = createdElements.filter((element) => element.tagName === "DT");
    expect(terms.map((element) => element.textContent)).toEqual(["Counter"]);
    expect(createdElements.filter((element) => element.tagName === "DD")[0]?.textContent).toBe("0");
    expect(operations).toEqual(["reference.read"]);
    expect(root.adoptedStyleSheets).toHaveLength(3);

    await unmount();
    await unmount();
    expect(root.adoptedStyleSheets).toEqual([sheetBefore]);
    expect(createdElements.find((element) => element.tagName === "ARTICLE")?.removed).toBe(true);
  });

  it("renders the main view when mounted with view main", async () => {
    vi.stubGlobal("CSSStyleSheet", FakeCSSStyleSheet);
    vi.stubGlobal("document", { createElement: (tagName: string) => fakeElement(tagName) });
    vi.stubGlobal("getComputedStyle", vi.fn(computedStyle));
    const root = { adoptedStyleSheets: [], append: vi.fn() } as unknown as ShadowRoot;

    const unmount = await mount(mountContext(root));

    expect(createdElements.find((element) => element.tagName === "H1")?.textContent).toBe("Reference Package");
    expect(createdElements.some((element) => element.textContent === "Increment counter")).toBe(true);
    await unmount();
  });
});
