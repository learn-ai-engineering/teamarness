import { afterEach, describe, expect, it, vi } from "vitest";
import {
  PACKAGE_REQUEST_ERROR_KIND,
  TEAMARNESS_THEME_TOKENS,
  adoptPackageStyles,
  describePackageError,
  installPackageUiKit,
  isAbortError,
  isPackageRequestError,
  packageErrorCode,
  typedPackageClient,
  type PackageClient,
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

/**
 * The input suppressor's MutationObserver holds the mount's shadow root, so a cleanup that does
 * not disconnect it leaks for as long as the Shell lives. The default `node` environment has no
 * `MutationObserver`, so these tests install a counting stub: without it the suppressor bails out
 * and every assertion below is vacuous.
 */
describe("Package UI kit input suppressor", () => {
  function stubObservers(): { constructed: number; live: number; disconnected: number } {
    const tally = { constructed: 0, live: 0, disconnected: 0 };
    class CountingMutationObserver {
      private connected = false;
      constructor() {
        tally.constructed += 1;
      }
      observe(): void {
        if (this.connected) return;
        this.connected = true;
        tally.live += 1;
      }
      disconnect(): void {
        tally.disconnected += 1;
        if (!this.connected) return;
        this.connected = false;
        tally.live -= 1;
      }
      takeRecords(): readonly never[] {
        return [];
      }
    }
    vi.stubGlobal("CSSStyleSheet", FakeCSSStyleSheet);
    vi.stubGlobal("MutationObserver", CountingMutationObserver);
    return tally;
  }

  it("leaves no live observer once the single installer cleans up", () => {
    const tally = stubObservers();
    const root = fakeShadowRoot();

    const cleanup = installPackageUiKit(root);
    expect(tally.live).toBe(1);

    cleanup();
    cleanup();
    expect(tally.live).toBe(0);
    expect(tally.disconnected).toBe(1);
  });

  it("leaves no live observer when overlapping mounts clean up in either order", () => {
    for (const order of ["first-then-second", "second-then-first"] as const) {
      const tally = stubObservers();
      const root = fakeShadowRoot();

      const cleanupFirst = installPackageUiKit(root);
      const cleanupSecond = installPackageUiKit(root);
      expect(tally.constructed, `${order}: one root needs one observer`).toBe(1);
      expect(tally.live).toBe(1);

      const [earlier, later] =
        order === "first-then-second" ? [cleanupFirst, cleanupSecond] : [cleanupSecond, cleanupFirst];
      earlier();
      expect(tally.live, `${order}: the surviving mount still needs the suppressor`).toBe(1);
      later();
      later();
      expect(tally.live, `${order}: the last cleanup must disconnect the observer`).toBe(0);

      // A leaked observer also poisons the root: the record that stops it is never removed, so a
      // later mount silently gets no suppressor at all.
      const cleanupThird = installPackageUiKit(root);
      expect(tally.constructed, `${order}: a fresh mount gets a fresh observer`).toBe(2);
      expect(tally.live).toBe(1);
      cleanupThird();
      expect(tally.live).toBe(0);
    }
  });

  it("leaves no live observer when the last cleanup cannot restore the stylesheets", () => {
    const tally = stubObservers();
    let sheets: readonly CSSStyleSheet[] = [];
    let sealed = false;
    const root = Object.defineProperty({}, "adoptedStyleSheets", {
      get: () => sheets,
      set: (next: readonly CSSStyleSheet[]) => {
        if (sealed) throw new Error("read only");
        sheets = next;
      },
    }) as ShadowRoot;

    const cleanup = installPackageUiKit(root);
    expect(tally.live).toBe(1);

    sealed = true;
    expect(cleanup).toThrow(/mutable ShadowRoot\.adoptedStyleSheets support/);
    expect(tally.live, "a failed sheet removal must not strand the observer").toBe(0);

    // And the root is still installable, rather than stuck on a spent installation record.
    sealed = false;
    const reinstalled = installPackageUiKit(root);
    expect(tally.constructed).toBe(2);
    reinstalled();
    expect(tally.live).toBe(0);
  });
});

describe("Package styles", () => {
  it("adopts one Package stylesheet and removes only it on idempotent cleanup", () => {
    vi.stubGlobal("CSSStyleSheet", FakeCSSStyleSheet);
    const unrelated = new FakeCSSStyleSheet() as unknown as CSSStyleSheet;
    const root = fakeShadowRoot([unrelated]);

    const cleanup = adoptPackageStyles(root, ".package { color: red; }");
    expect(root.adoptedStyleSheets).toHaveLength(2);
    const adopted = root.adoptedStyleSheets[1] as unknown as FakeCSSStyleSheet;
    expect(adopted.cssText).toBe(".package { color: red; }");

    cleanup();
    cleanup();
    expect(root.adoptedStyleSheets).toEqual([unrelated]);
  });

  it("fails with a Package-neutral message when constructed stylesheets are unavailable", () => {
    vi.stubGlobal("CSSStyleSheet", undefined);
    expect(() => adoptPackageStyles(fakeShadowRoot(), "")).toThrow(
      "Package styles require constructed CSSStyleSheet support",
    );

    vi.stubGlobal("CSSStyleSheet", FakeCSSStyleSheet);
    expect(() => adoptPackageStyles({} as ShadowRoot, "")).toThrow(
      "Package styles require ShadowRoot.adoptedStyleSheets support",
    );
  });
});

describe("typed client", () => {
  it("typed client is the same object as the untyped client", () => {
    const client = { request: async () => ({}), subscribe: async () => ({ close: async () => {} }) } as unknown as PackageClient;
    expect(typedPackageClient(client)).toBe(client);
  });
});

describe("author surface", () => {
  it("exports only the author contract and no transport or Tauri host pieces", async () => {
    const surface = await import("./index");
    expect(Object.keys(surface).sort()).toEqual([
      "PACKAGE_REQUEST_ERROR_KIND",
      "TEAMARNESS_THEME_TOKENS",
      "adoptPackageStyles",
      "describePackageError",
      "installPackageUiKit",
      "isAbortError",
      "isPackageRequestError",
      "packageErrorCode",
      "typedPackageClient",
    ]);
  });
});

describe("Package request errors", () => {
  const hostError = {
    kind: PACKAGE_REQUEST_ERROR_KIND,
    version: 2,
    namespace: "teamarness.package-host",
    code: "backend_error",
    stage: "ui",
    retryable: false,
    recovery: "none",
    diagnosticId: "diag-1",
    packageError: null,
  };
  const packageError = {
    ...hostError,
    packageError: { code: "reference.requested_failure", retryable: false, details: { reason: "a private detail" } },
  };

  it("recognises only complete branded Package request errors", () => {
    expect(isPackageRequestError(hostError)).toBe(true);
    expect(isPackageRequestError(packageError)).toBe(true);
    for (const invalid of [
      { ...hostError, kind: "other" },
      { ...hostError, retryable: "no" },
      { ...hostError, version: "2" },
      { ...hostError, packageError: { code: "a", retryable: false, details: [] } },
      { ...hostError, packageError: { code: 1, retryable: false, details: {} } },
      new Error("boom"),
      null,
      "backend_error",
    ]) {
      expect(isPackageRequestError(invalid)).toBe(false);
    }
  });

  it("describes Package codes before host codes and never includes details", () => {
    expect(describePackageError(packageError)).toBe("reference.requested_failure");
    expect(describePackageError(hostError)).toBe("backend_error (ui)");
    expect(describePackageError(new Error("the mount closed"))).toBe("the mount closed");
    expect(describePackageError(undefined)).toBe("Request failed");
    expect(describePackageError(packageError)).not.toContain("a private detail");
    expect(packageErrorCode(packageError)).toBe("reference.requested_failure");
    expect(packageErrorCode(hostError)).toBeNull();
    expect(packageErrorCode(new Error("boom"))).toBeNull();
  });

  it("recognises abort errors by name", () => {
    expect(isAbortError(new DOMException("cancelled", "AbortError"))).toBe(true);
    expect(isAbortError({ name: "AbortError" })).toBe(true);
    expect(isAbortError(new Error("boom"))).toBe(false);
    expect(isAbortError(null)).toBe(false);
  });
});
