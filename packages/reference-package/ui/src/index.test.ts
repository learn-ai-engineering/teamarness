import { describe, expect, it } from "vitest";

import { mount } from "./index";

describe("reference Package UI", () => {
  it("uses only its mount-scoped client for Package operations", () => {
    const source = mount.toString();
    expect(source).toContain('client.request("reference.read"');
    expect(source).toContain('client.request("reference.increment"');
    expect(source).toContain('client.subscribe("reference.changes"');
    expect(source).not.toContain("invoke(");
    expect(source).not.toContain("document.head");
  });

  it("renders the generic Package and Workspace presentation fields", () => {
    const source = mount.toString();
    for (const field of ["source", "trust", "lifecycle", "processGeneration", "workspace.name"]) {
      expect(source).toContain(field);
    }
  });
});
