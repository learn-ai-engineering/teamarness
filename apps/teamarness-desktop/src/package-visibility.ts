import type { PackageInstanceKey } from "./package-contracts";

export const INSTALLATION_HIDDEN_PACKAGES_STORAGE_KEY = "teamarness.installation.hidden-packages";

export interface PackageVisibilityStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

// Hiding is an Installation-local navigation preference; it never changes the Package Instance itself.
export interface InstallationPackageVisibility {
  isHidden(instance: PackageInstanceKey): boolean;
  setHidden(instance: PackageInstanceKey, hidden: boolean): void;
}

type InstallationPackageVisibilityOptions = Readonly<{
  storage?: PackageVisibilityStorage | null;
}>;

function browserStorage(): PackageVisibilityStorage | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null;
  }
}

function instanceEntry(instance: PackageInstanceKey): string {
  return `${instance.workspaceId}/${instance.packageIdentity}`;
}

function readHidden(storage: PackageVisibilityStorage | null): Set<string> {
  try {
    const parsed: unknown = JSON.parse(storage?.getItem(INSTALLATION_HIDDEN_PACKAGES_STORAGE_KEY) ?? "[]");
    return new Set(Array.isArray(parsed) ? parsed.filter((entry): entry is string => typeof entry === "string") : []);
  } catch {
    return new Set();
  }
}

export function createInstallationPackageVisibility(
  options: InstallationPackageVisibilityOptions = {},
): InstallationPackageVisibility {
  const storage = options.storage === undefined ? browserStorage() : options.storage;
  const hidden = readHidden(storage);

  return Object.freeze({
    isHidden: (instance: PackageInstanceKey) => hidden.has(instanceEntry(instance)),
    setHidden(instance: PackageInstanceKey, nextHidden: boolean) {
      if (nextHidden) hidden.add(instanceEntry(instance));
      else hidden.delete(instanceEntry(instance));
      try {
        storage?.setItem(INSTALLATION_HIDDEN_PACKAGES_STORAGE_KEY, JSON.stringify([...hidden]));
      } catch {
        // The choice still applies for this session when storage is unavailable.
      }
    },
  });
}
