/// <reference types="node" />
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const REPOSITORY = join(import.meta.dirname, "../../../../..");
const WORKSPACE = "packages/documents-package/ui";

type LockEntry = { version?: string; dependencies?: Record<string, string>; devDependencies?: Record<string, string>; optionalDependencies?: Record<string, string> };

/** Every package the lockfile resolves for the Documents UI, keyed by name, with the versions seen. */
function dependencyTree(): Map<string, Set<string>> {
  const lock = JSON.parse(readFileSync(join(REPOSITORY, "package-lock.json"), "utf8")) as { packages: Record<string, LockEntry> };
  const seen = new Map<string, Set<string>>();
  const visited = new Set<string>();
  /** Node's resolution in the lockfile: the nearest `node_modules/<name>` above `from`. */
  const resolve = (from: string, name: string): string | null => {
    let base = from;
    for (;;) {
      const candidate = base === "" ? `node_modules/${name}` : `${base}/node_modules/${name}`;
      if (lock.packages[candidate] !== undefined) return candidate;
      if (base === "") return null;
      const cut = base.lastIndexOf("/node_modules/");
      base = cut >= 0 ? base.slice(0, cut) : base.startsWith("node_modules/") ? "" : base.includes("/") ? base.slice(0, base.lastIndexOf("/")) : "";
    }
  };
  const walk = (path: string, includeDev: boolean): void => {
    if (visited.has(path)) return;
    visited.add(path);
    const entry = lock.packages[path]!;
    const names = {
      ...entry.dependencies, ...entry.optionalDependencies, ...(includeDev ? entry.devDependencies : {}),
    };
    for (const name of Object.keys(names)) {
      const resolved = resolve(path, name);
      if (resolved === null) continue;
      const versions = seen.get(name) ?? new Set<string>();
      versions.add(lock.packages[resolved]!.version ?? "");
      seen.set(name, versions);
      walk(resolved, false);
    }
  };
  walk(WORKSPACE, true);
  return seen;
}

describe("the Documents UI dependency tree", () => {
  const tree = dependencyTree();

  it("holds the binding and one copy of each ProseMirror package", () => {
    expect(tree.has("loro-prosemirror")).toBe(true);
    for (const [name, versions] of tree) {
      if (name.startsWith("prosemirror-")) expect([...versions], name).toHaveLength(1);
    }
  });

  it("does not include prosemirror-history: undo is LoroUndoPlugin's, per person", () => {
    expect(tree.has("prosemirror-history")).toBe(false);
  });
});
