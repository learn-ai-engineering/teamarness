import { describe, expect, it } from "vitest";
import { TEAMARNESS_THEME_TOKENS } from "@teamarness/package-ui-sdk";
import { browserThemeSource, type ThemeMediaQuery } from "./package-theme";
import type { ThemeSnapshot } from "./package-frame/protocol";

function environment(initial: Readonly<{ scheme: string; dark: boolean }>): {
  source: ReturnType<typeof browserThemeSource>;
  values: Record<string, string>;
  scheme: { value: string };
  media: ThemeMediaQuery & { matches: boolean; fire(): void; listeners: number };
  changePreference(): void;
} {
  const values: Record<string, string> = {};
  for (const [index, token] of TEAMARNESS_THEME_TOKENS.entries()) values[token] = `  value-${index}  `;
  const scheme = { value: initial.scheme };
  const listeners = new Set<() => void>();
  const media = {
    matches: initial.dark,
    addEventListener: (_type: "change", listener: () => void) => { listeners.add(listener); },
    removeEventListener: (_type: "change", listener: () => void) => { listeners.delete(listener); },
    fire: () => { for (const listener of [...listeners]) listener(); },
    get listeners() { return listeners.size; },
  };
  const preferenceListeners = new Set<() => void>();
  const root = {} as Element;
  const source = browserThemeSource({
    root,
    getComputedStyle: (element) => {
      expect(element).toBe(root);
      return {
        getPropertyValue: (property) => (property === "color-scheme" ? scheme.value : values[property] ?? ""),
      };
    },
    matchMedia: (query) => {
      expect(query).toBe("(prefers-color-scheme: dark)");
      return media;
    },
    preferences: {
      subscribe: (listener) => {
        preferenceListeners.add(listener);
        return () => preferenceListeners.delete(listener);
      },
    },
  });
  return {
    source,
    values,
    scheme,
    media,
    changePreference: () => { for (const listener of [...preferenceListeners]) listener(); },
  };
}

describe("Shell theme source for Package frames", () => {
  it("reads every token value from the Shell root with its color scheme", () => {
    const light = environment({ scheme: "light", dark: true });
    const snapshot = light.source.current();
    expect(Object.keys(snapshot.values)).toEqual([...TEAMARNESS_THEME_TOKENS]);
    expect(snapshot.values["--teamarness-color-background"]).toBe("value-0");
    expect(snapshot.colorScheme).toBe("light");
    expect(Object.isFrozen(snapshot)).toBe(true);

    expect(environment({ scheme: "dark", dark: false }).source.current().colorScheme).toBe("dark");
    expect(environment({ scheme: "normal", dark: true }).source.current().colorScheme).toBe("dark");
    expect(environment({ scheme: "", dark: false }).source.current().colorScheme).toBe("light");
  });

  it("notifies on preference changes and system appearance changes and stops after unsubscribe", () => {
    const test = environment({ scheme: "light", dark: false });
    const seen: ThemeSnapshot[] = [];
    const stop = test.source.subscribe((theme) => seen.push(theme));

    test.changePreference();
    expect(seen).toHaveLength(1);

    test.scheme.value = "dark";
    test.media.fire();
    expect(seen).toHaveLength(2);
    expect(seen[1]?.colorScheme).toBe("dark");

    stop();
    stop();
    test.changePreference();
    test.media.fire();
    expect(seen).toHaveLength(2);
    expect(test.media.listeners).toBe(0);
  });
});
