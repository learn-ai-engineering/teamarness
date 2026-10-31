import { expect, it } from "vitest";
import fixture from "../contracts/fixtures/package-bridge.json";

it("keeps wide event sequences as decimal strings and bridge fixtures path-free", () => {
  expect(fixture.event.sequence).toBe("9007199254740993");
  const encoded = JSON.stringify(fixture);
  for (const forbidden of ["sourcePath", "cachePath", "dataPath", "processHandle", "secret", "transportHandle"]) {
    expect(encoded).not.toContain(forbidden);
  }
});

it("reports whether a mount may nest content frames", () => {
  expect(fixture.loader.nestedContent).toBe(false);
});
