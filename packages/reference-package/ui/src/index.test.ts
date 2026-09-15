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

  it("adopts the shared theme contract and optional UI kit", () => {
    const source = mount.toString();
    expect(source).toContain("TEAMARNESS_THEME_TOKENS");
    expect(source).toContain("installPackageUiKit");
    expect(source).toContain("teamarness-control");
    expect(source).toContain("teamarness-panel");
  });

  it("keeps qualification state while rendering only display-safe readiness labels", () => {
    const source = mount.toString();
    for (const label of ["Package UI", "Workspace", "Theme contract", "Membership access", "Signing round trip"]) {
      expect(source).toContain(label);
    }
    for (const diagnostic of [
      "package.identity",
      "workspace.id",
      "package.source",
      "package.trust",
      "package.lifecycle",
      "package.processGeneration",
      "membershipVersion",
      "processGeneration",
    ]) {
      expect(source).not.toContain(diagnostic);
    }
  });
});
