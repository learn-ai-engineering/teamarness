// @vitest-environment happy-dom

import { describe, expect, it } from "vitest";
import { renderMarkdown } from "./markdown";

// markdown.test.ts runs without a DOM, where renderMarkdown skips DOMPurify.
// These cases need a window so the sanitizer's allowlist is what is tested.
describe("sanitized Conversations Markdown", () => {
  it("keeps GitHub-style tables, without their alignment styles", () => {
    const host = document.createElement("div");
    host.innerHTML = renderMarkdown("| Name | Count |\n| :--- | ---: |\n| **apples** | 3 |\n| pears | 5 |");
    const table = host.querySelector("table");
    expect(table).not.toBeNull();
    expect(Array.from(host.querySelectorAll("thead th"), (cell) => cell.textContent)).toEqual(["Name", "Count"]);
    expect(Array.from(host.querySelectorAll("tbody tr"), (row) => Array.from(row.querySelectorAll("td"), (cell) => cell.textContent))).toEqual([
      ["apples", "3"],
      ["pears", "5"],
    ]);
    expect(host.querySelector("tbody td strong")?.textContent).toBe("apples");
    expect(host.querySelector("[style]")).toBeNull();
  });

  it("still strips unsafe content inside table cells", () => {
    const host = document.createElement("div");
    host.innerHTML = renderMarkdown("| a | b |\n| - | - |\n| <script>alert(1)</script> | [x](javascript:alert(1)) ![i](https://example.invalid/i.png) |");
    expect(host.querySelector("table")).not.toBeNull();
    expect(host.querySelector("script, img")).toBeNull();
    expect(host.innerHTML).not.toContain("javascript:");
  });
});
