/// <reference types="node" />
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { LoroDoc } from "loro-crdt";
import { createNodeFromLoroObj } from "loro-prosemirror";
import type { Node } from "prosemirror-model";
import { afterEach, describe, expect, it, vi } from "vitest";
import { MARK_EXPAND, schema } from "./schema";

// The committed body trees; `backend/tests/cross_language.rs` documents how to regenerate them.
const TREES = join(import.meta.dirname, "../../../fixtures/body-trees");

function fixtures(directory: string): string[] {
  return readdirSync(join(TREES, directory)).filter((file) => file.endsWith(".loro")).sort();
}

/** Reads a snapshot through the binding's own reader, failing on anything it logs. */
function read(directory: string, file: string): Node {
  const errors = vi.spyOn(console, "error").mockImplementation(() => {});
  const doc = LoroDoc.fromSnapshot(new Uint8Array(readFileSync(join(TREES, directory, file))));
  const node = createNodeFromLoroObj(schema, doc.getMap("doc") as never, new Map());
  expect(errors, `${file}: the binding refused part of the tree`).not.toHaveBeenCalled();
  return node;
}

describe("the shared body schema across languages", () => {
  afterEach(() => { vi.restoreAllMocks(); });

  it("reads every Rust-written tree through the binding as a schema-valid document", () => {
    const files = fixtures("from-rust");
    expect(files.length).toBeGreaterThanOrEqual(19);
    const seen = { nodes: new Set<string>(), marks: new Set<string>() };
    for (const file of files) {
      const node = read("from-rust", file);
      expect(() => node.check(), file).not.toThrow();
      node.descendants((child) => {
        seen.nodes.add(child.type.name);
        for (const mark of child.marks) seen.marks.add(mark.type.name);
      });
    }
    // Every node and mark of the schema is covered by some Rust-written tree.
    const nodes = Object.keys(schema.nodes).filter((name) => name !== "doc");
    expect([...seen.nodes].sort()).toEqual(nodes.sort());
    expect([...seen.marks].sort()).toEqual(Object.keys(schema.marks).sort());
  });

  it("reads the Rust genesis as one empty paragraph", () => {
    expect(read("from-rust", "00-genesis.loro").toJSON()).toEqual({ type: "doc", content: [{ type: "paragraph" }] });
  });

  it("reads every binding-written snapshot back to the document the binding wrote", () => {
    const files = fixtures("from-binding");
    expect(files.length).toBeGreaterThanOrEqual(8);
    for (const file of files) {
      const node = read("from-binding", file);
      expect(() => node.check(), file).not.toThrow();
      const recorded: unknown = JSON.parse(readFileSync(join(TREES, "from-binding", file.replace(/\.loro$/, ".pm.json")), "utf8"));
      expect(node.toJSON(), file).toEqual(recorded);
    }
  });

  it("configures mark expansion exactly as the binding derives it from the schema", () => {
    const derived = Object.fromEntries(Object.entries(schema.marks).map(([name, type]) =>
      [name, { expand: type.spec.inclusive !== false ? "after" : "none" }]));
    expect(MARK_EXPAND).toEqual(derived);
    expect(MARK_EXPAND.link).toEqual({ expand: "none" });
  });

  it("has no list spacing attribute", () => {
    expect(Object.keys(schema.nodes.bullet_list!.spec.attrs ?? {})).toEqual([]);
    expect(Object.keys(schema.nodes.ordered_list!.spec.attrs ?? {})).toEqual(["order"]);
    expect(schema.nodes.list_item!.spec.content).toBe("paragraph block*");
  });
});
