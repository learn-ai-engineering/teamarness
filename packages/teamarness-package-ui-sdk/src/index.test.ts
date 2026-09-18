import { afterEach, describe, expect, it, vi } from "vitest";
import {
  installPackageUiKit,
  TEAMARNESS_THEME_TOKENS,
  type TeamarnessThemeToken,
} from "./index";

const EXPECTED_THEME_TOKENS = [
  "--teamarness-color-background",
  "--teamarness-color-panel",
  "--teamarness-color-text",
  "--teamarness-color-text-muted",
  "--teamarness-color-border",
  "--teamarness-color-accent",
  "--teamarness-color-danger",
  "--teamarness-color-focus",
  "--teamarness-font-body",
  "--teamarness-font-mono",
  "--teamarness-radius-control",
  "--teamarness-radius-panel",
] as const satisfies readonly TeamarnessThemeToken[];

class FakeCSSStyleSheet {
  cssText = "";

  replaceSync(css: string): void {
    this.cssText = css;
  }
}

function fakeShadowRoot(initialSheets: CSSStyleSheet[] = []): ShadowRoot {
  return { adoptedStyleSheets: initialSheets } as unknown as ShadowRoot;
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("Package presentation contract", () => {
  it("publishes the exact frozen theme-token authority", () => {
    expect(TEAMARNESS_THEME_TOKENS).toEqual(EXPECTED_THEME_TOKENS);
    expect(Object.isFrozen(TEAMARNESS_THEME_TOKENS)).toBe(true);
  });

  it("adopts one class-scoped kit sheet and cleans up overlapping installers safely", () => {
    vi.stubGlobal("CSSStyleSheet", FakeCSSStyleSheet);
    const packageSheetBefore = new FakeCSSStyleSheet() as unknown as CSSStyleSheet;
    const packageSheetAfter = new FakeCSSStyleSheet() as unknown as CSSStyleSheet;
    const root = fakeShadowRoot([packageSheetBefore]);
    const cleanupFirst = installPackageUiKit(root);
    const cleanupSecond = installPackageUiKit(root);

    expect(root.adoptedStyleSheets).toHaveLength(2);
    expect(root.adoptedStyleSheets[0]).toBe(packageSheetBefore);
    const kitSheet = root.adoptedStyleSheets[1] as unknown as FakeCSSStyleSheet;
    expect(kitSheet.cssText).toMatch(/\.teamarness-control\s*\{[^}]*font:\s*inherit/);
    for (const className of [
      "teamarness-control",
      "teamarness-control--primary",
      "teamarness-control--secondary",
      "teamarness-field",
      "teamarness-input",
      "teamarness-help",
      "teamarness-validation",
      "teamarness-notice",
      "teamarness-notice--danger",
      "teamarness-panel",
      "teamarness-stack",
      "teamarness-inline",
      "teamarness-focus",
    ]) {
      expect(kitSheet.cssText).toContain(`.${className}`);
    }
    for (const token of TEAMARNESS_THEME_TOKENS) expect(kitSheet.cssText).toContain(`var(${token})`);

    root.adoptedStyleSheets = [...root.adoptedStyleSheets, packageSheetAfter];
    cleanupFirst();
    cleanupFirst();
    expect(root.adoptedStyleSheets).toEqual([packageSheetBefore, kitSheet, packageSheetAfter]);
    cleanupSecond();
    cleanupSecond();
    expect(root.adoptedStyleSheets).toEqual([packageSheetBefore, packageSheetAfter]);
  });

  it("fails rather than adding an inline fallback when constructed sheets are unsupported", () => {
    vi.stubGlobal("CSSStyleSheet", undefined);
    expect(() => installPackageUiKit(fakeShadowRoot())).toThrow(/constructed CSSStyleSheet support/);

    vi.stubGlobal("CSSStyleSheet", class {});
    expect(() => installPackageUiKit(fakeShadowRoot())).toThrow(/replaceSync support/);

    vi.stubGlobal("CSSStyleSheet", FakeCSSStyleSheet);
    expect(() => installPackageUiKit({} as ShadowRoot)).toThrow(/adoptedStyleSheets support/);
    const immutableRoot = Object.defineProperty({}, "adoptedStyleSheets", {
      get: () => [],
      set: () => { throw new Error("read only"); },
    }) as ShadowRoot;
    expect(() => installPackageUiKit(immutableRoot)).toThrow(/mutable ShadowRoot\.adoptedStyleSheets support/);
  });
});

describe("author surface", () => {
  it("exports only the author contract and no transport or Tauri host pieces", async () => {
    const surface = await import("./index");
    expect(Object.keys(surface).sort()).toEqual(["TEAMARNESS_THEME_TOKENS", "installPackageUiKit"]);
  });
});
