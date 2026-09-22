import { describe, expect, it } from "vitest";
import {
  createInstallationThemeController,
  INSTALLATION_THEME_STORAGE_KEY,
  type ThemePreference,
  type ThemeRoot,
  type ThemeStorage,
} from "./theme";

class TestRoot implements ThemeRoot {
  public readonly attributes = new Map<string, string>();

  public setAttribute(name: string, value: string): void {
    this.attributes.set(name, value);
  }

  public theme(): string | undefined {
    return this.attributes.get("data-theme");
  }
}

function storageWith(value: string | null): {
  storage: ThemeStorage;
  reads: string[];
  writes: Array<readonly [string, string]>;
} {
  const reads: string[] = [];
  const writes: Array<readonly [string, string]> = [];
  let storedValue = value;
  return {
    storage: {
      getItem(key) {
        reads.push(key);
        return storedValue;
      },
      setItem(key, next) {
        writes.push([key, next]);
        storedValue = next;
      },
    },
    reads,
    writes,
  };
}

describe("Installation theme preference", () => {
  it.each<ThemePreference>(["system", "light", "dark"])(
    "restores and applies a persisted %s preference",
    (preference) => {
      const root = new TestRoot();
      const stored = storageWith(preference);

      const controller = createInstallationThemeController({ root, storage: stored.storage });

      expect(controller.getPreference()).toBe(preference);
      expect(root.theme()).toBe(preference);
      expect(stored.reads).toEqual([INSTALLATION_THEME_STORAGE_KEY]);
      expect(stored.writes).toEqual([]);
    },
  );

  it("persists a valid selection and updates the stable root", () => {
    const root = new TestRoot();
    const stored = storageWith(null);
    const controller = createInstallationThemeController({ root, storage: stored.storage });

    expect(controller.getPreference()).toBe("system");
    expect(root.theme()).toBe("system");

    controller.selectPreference("dark");

    expect(controller.getPreference()).toBe("dark");
    expect(root.theme()).toBe("dark");
    expect(stored.writes).toEqual([[INSTALLATION_THEME_STORAGE_KEY, "dark"]]);

    const restoredRoot = new TestRoot();
    const restored = createInstallationThemeController({ root: restoredRoot, storage: stored.storage });
    expect(restored.getPreference()).toBe("dark");
    expect(restoredRoot.theme()).toBe("dark");
  });

  it.each([null, "sepia", "DARK"])("falls back to System for a missing or invalid stored value (%s)", (value) => {
    const root = new TestRoot();
    const controller = createInstallationThemeController({ root, storage: storageWith(value).storage });

    expect(controller.getPreference()).toBe("system");
    expect(root.theme()).toBe("system");
  });

  it("falls back to System when storage is unavailable or reading fails", () => {
    const unavailableRoot = new TestRoot();
    const unavailable = createInstallationThemeController({ root: unavailableRoot, storage: null });
    expect(unavailable.getPreference()).toBe("system");
    expect(unavailableRoot.theme()).toBe("system");

    const failingRoot = new TestRoot();
    const failingStorage: ThemeStorage = {
      getItem() { throw new Error("read denied"); },
      setItem() { throw new Error("write denied"); },
    };
    expect(() => createInstallationThemeController({ root: failingRoot, storage: failingStorage })).not.toThrow();
    expect(failingRoot.theme()).toBe("system");
  });

  it("notifies subscribers after a preference change and stops after unsubscribe", () => {
    const root = new TestRoot();
    const stored = storageWith(null);
    const controller = createInstallationThemeController({ root, storage: stored.storage });
    const seen: ThemePreference[] = [];
    const stop = controller.subscribe((preference) => seen.push(preference));

    controller.selectPreference("dark");
    controller.selectPreference("light");
    expect(seen).toEqual(["dark", "light"]);
    expect(root.theme()).toBe("light");

    stop();
    stop();
    controller.selectPreference("system");
    expect(seen).toEqual(["dark", "light"]);
  });

  it("contains a write failure and resolves the selection to System", () => {
    const root = new TestRoot();
    const storage: ThemeStorage = {
      getItem: () => "light",
      setItem: () => { throw new Error("write denied"); },
    };
    const controller = createInstallationThemeController({ root, storage });

    expect(() => controller.selectPreference("dark")).not.toThrow();
    expect(controller.getPreference()).toBe("system");
    expect(root.theme()).toBe("system");
  });
});
