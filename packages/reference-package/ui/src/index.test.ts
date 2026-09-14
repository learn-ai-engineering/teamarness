import { describe, expect, it } from "vitest";

describe("reference Package UI build shell", () => {
  it("contains no Package-owned operation until Phase 6", async () => {
    const source = await import("./index");
    expect(typeof source.mount).toBe("function");
    expect(source.mount.toString()).not.toContain("context.client.");
    expect(source.mount.toString()).not.toContain("document.head");
  });
});
