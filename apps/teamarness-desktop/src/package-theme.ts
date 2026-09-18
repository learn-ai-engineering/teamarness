import { TEAMARNESS_THEME_TOKENS, type TeamarnessThemeToken } from "@teamarness/package-ui-sdk";
import type { ThemeSnapshot } from "./package-frame/protocol";

export interface PackageThemeSource {
  current(): ThemeSnapshot;
  subscribe(listener: (theme: ThemeSnapshot) => void): () => void;
}

export type ThemeMediaQuery = Readonly<{
  matches: boolean;
  addEventListener(type: "change", listener: () => void): void;
  removeEventListener(type: "change", listener: () => void): void;
}>;

export type ThemeStyleReader = Readonly<{ getPropertyValue(property: string): string }>;

export type BrowserThemeEnvironment = Readonly<{
  root: Element;
  getComputedStyle(element: Element): ThemeStyleReader;
  matchMedia(query: string): ThemeMediaQuery;
  preferences: Readonly<{ subscribe(listener: () => void): () => void }>;
}>;

const DARK_QUERY = "(prefers-color-scheme: dark)";

/** Reads the Shell's own theme token values so the bridge can hand them to a Package frame. */
export function browserThemeSource(environment: BrowserThemeEnvironment): PackageThemeSource {
  const read = (): ThemeSnapshot => {
    const style = environment.getComputedStyle(environment.root);
    const values: Record<string, string> = {};
    for (const token of TEAMARNESS_THEME_TOKENS) values[token] = style.getPropertyValue(token).trim();
    const scheme = style.getPropertyValue("color-scheme").trim().toLowerCase();
    const dark = scheme === "dark" || (scheme !== "light" && environment.matchMedia(DARK_QUERY).matches);
    return Object.freeze({
      colorScheme: dark ? "dark" : "light",
      values: Object.freeze(values) as Readonly<Record<TeamarnessThemeToken, string>>,
    });
  };

  return Object.freeze({
    current: read,
    subscribe(listener: (theme: ThemeSnapshot) => void): () => void {
      const notify = (): void => { listener(read()); };
      const media = environment.matchMedia(DARK_QUERY);
      media.addEventListener("change", notify);
      const stopPreferences = environment.preferences.subscribe(notify);
      let stopped = false;
      return () => {
        if (stopped) return;
        stopped = true;
        media.removeEventListener("change", notify);
        stopPreferences();
      };
    },
  });
}
