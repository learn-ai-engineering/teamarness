export const INSTALLATION_THEME_STORAGE_KEY = "teamarness.installation.theme";

export type ThemePreference = "system" | "light" | "dark";

export interface ThemeStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

export interface ThemeRoot {
  setAttribute(name: string, value: string): void;
}

export interface InstallationThemeController {
  getPreference(): ThemePreference;
  selectPreference(preference: ThemePreference): void;
  subscribe(listener: (preference: ThemePreference) => void): () => void;
}

type InstallationThemeOptions = Readonly<{
  storage?: ThemeStorage | null;
  root?: ThemeRoot;
}>;

function isThemePreference(value: string | null): value is ThemePreference {
  return value === "system" || value === "light" || value === "dark";
}

function browserStorage(): ThemeStorage | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null;
  }
}

export function createInstallationThemeController(
  options: InstallationThemeOptions = {},
): InstallationThemeController {
  const root = options.root ?? document.documentElement;
  const storage = options.storage === undefined ? browserStorage() : options.storage;
  let preference: ThemePreference = "system";

  try {
    const stored = storage?.getItem(INSTALLATION_THEME_STORAGE_KEY) ?? null;
    if (isThemePreference(stored)) preference = stored;
  } catch {
    preference = "system";
  }
  root.setAttribute("data-theme", preference);
  const listeners = new Set<(preference: ThemePreference) => void>();

  return Object.freeze({
    getPreference: () => preference,
    selectPreference(nextPreference: ThemePreference) {
      try {
        if (storage === null) throw new Error("Theme storage is unavailable");
        storage.setItem(INSTALLATION_THEME_STORAGE_KEY, nextPreference);
        preference = nextPreference;
      } catch {
        preference = "system";
      }
      root.setAttribute("data-theme", preference);
      for (const listener of [...listeners]) listener(preference);
    },
    subscribe(listener: (preference: ThemePreference) => void): () => void {
      listeners.add(listener);
      return () => { listeners.delete(listener); };
    },
  });
}
