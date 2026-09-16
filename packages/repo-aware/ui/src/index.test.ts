import { describe, it, expect } from "vitest";

describe("Repo-aware Package UI", () => {
  it("exports mount function", async () => {
    const mod = await import("./index");
    expect(typeof mod.mount).toBe("function");
  });
});