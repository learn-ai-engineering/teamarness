import { describe, expect, it } from "vitest";
import type { PackageInstanceKey } from "./package-contracts";
import {
  createInstallationPackageVisibility,
  type PackageVisibilityStorage,
} from "./package-visibility";

const one: PackageInstanceKey = { packageIdentity: "com.example.one", workspaceId: "aa" };
const oneElsewhere: PackageInstanceKey = { ...one, workspaceId: "bb" };

function memoryStorage(initial: string | null = null): PackageVisibilityStorage & { value: string | null } {
  return {
    value: initial,
    getItem() { return this.value; },
    setItem(_key, next) { this.value = next; },
  };
}

describe("Installation Package visibility", () => {
  it("persists hidden Package Instances per Workspace across controllers", () => {
    const storage = memoryStorage();
    createInstallationPackageVisibility({ storage }).setHidden(one, true);

    const restored = createInstallationPackageVisibility({ storage });
    expect(restored.isHidden(one)).toBe(true);
    expect(restored.isHidden(oneElsewhere)).toBe(false);

    restored.setHidden(one, false);
    expect(createInstallationPackageVisibility({ storage }).isHidden(one)).toBe(false);
  });

  it("treats malformed or unavailable storage as all visible", () => {
    expect(createInstallationPackageVisibility({ storage: memoryStorage("{not json") }).isHidden(one)).toBe(false);
    const throwing: PackageVisibilityStorage = {
      getItem() { throw new Error("blocked"); },
      setItem() { throw new Error("blocked"); },
    };
    const visibility = createInstallationPackageVisibility({ storage: throwing });
    expect(visibility.isHidden(one)).toBe(false);
    visibility.setHidden(one, true);
    expect(visibility.isHidden(one)).toBe(true);
  });
});
