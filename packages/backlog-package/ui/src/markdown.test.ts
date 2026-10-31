// @vitest-environment happy-dom

import { describe, expect, it } from "vitest";
import { renderMarkdown, sanitizeHtml } from "./markdown";

function fragment(html: string): HTMLElement {
  const element = document.createElement("div");
  element.innerHTML = html;
  return element;
}

describe("Backlog Markdown", () => {
  it("renders the supported formatting and escapes raw HTML", () => {
    const html = renderMarkdown("# Heading\n\n**bold** and *emphasis* ~~gone~~\n\n##### Five\n\n###### Six\n\n<script>alert(1)</script>");
    expect(html).toContain("<h1>Heading</h1>");
    expect(html).toContain("<strong>bold</strong>");
    expect(html).toContain("<em>emphasis</em>");
    expect(html).toContain("<s>gone</s>");
    expect(html).toContain("<h5>Five</h5>");
    expect(html).toContain("<h6>Six</h6>");
    expect(html).not.toContain("<script>");
    expect(html).toContain("&lt;script&gt;");
  });

  it("renders tables with column alignment and no inline style", () => {
    const html = renderMarkdown("| Name | Count |\n| :--- | ---: |\n| Alpha | 1 |\n| Beta | 2 |");
    const table = fragment(html).querySelector("table");
    expect(table).not.toBeNull();
    expect([...table!.querySelectorAll("thead th")].map((cell) => cell.textContent)).toEqual(["Name", "Count"]);
    expect([...table!.querySelectorAll("tbody tr")].map((row) => [...row.children].map((cell) => cell.textContent)))
      .toEqual([["Alpha", "1"], ["Beta", "2"]]);
    expect(table!.querySelector("th")?.getAttribute("align")).toBe("left");
    expect(table!.querySelector("td:last-child")?.getAttribute("align")).toBe("right");
    expect(html).not.toContain("style=");
  });

  it("renders task list items as read-only checkboxes", () => {
    const html = renderMarkdown("- [ ] open\n- [x] done\n- [X] also done\n- plain\n- [y] not a task");
    const items = [...fragment(html).querySelectorAll("li")];
    const boxes = items.map((item) => item.querySelector("input"));
    expect(boxes.slice(0, 3).every((box) => box?.type === "checkbox" && box.disabled)).toBe(true);
    expect(boxes.slice(0, 3).map((box) => box?.checked)).toEqual([false, true, true]);
    expect(items.slice(0, 3).map((item) => item.textContent?.trim())).toEqual(["open", "done", "also done"]);
    expect(boxes[3]).toBeNull();
    expect(boxes[4]).toBeNull();
    expect(items[4]?.textContent).toBe("[y] not a task");
  });

  it("renders task items in loose lists and nested lists", () => {
    const html = renderMarkdown("- [x] first\n\n- [ ] second\n  - [x] nested");
    const boxes = [...fragment(html).querySelectorAll("input")];
    expect(boxes).toHaveLength(3);
    expect(boxes.map((box) => box.checked)).toEqual([true, false, true]);
  });

  it("strips non-checkbox inputs, scripts, event handlers and javascript: links", () => {
    const html = renderMarkdown("<input type=\"text\"> [bad](javascript:alert(1)) [also bad](%6a%61vascript:alert(1))");
    expect(fragment(html).querySelector("input")).toBeNull();
    expect(html).not.toContain("href=\"javascript");
    expect(html).not.toContain("javascript:alert");

    // One case per call: happy-dom's NodeIterator skips the sibling after a node DOMPurify removes,
    // which browsers do not, so adjacent forbidden nodes would hide each other in this test.
    const clean = (html: string) => fragment(sanitizeHtml(html));
    expect(clean("<p><input type=\"text\" value=\"x\"> <input type=\"checkbox\" checked onclick=\"alert(1)\"></p>")
      .querySelectorAll("input")).toHaveLength(1);
    const checkbox = clean("<p><input type=\"text\"> <input type=\"checkbox\" checked onclick=\"alert(1)\"></p>").querySelector("input");
    expect(checkbox?.type).toBe("checkbox");
    expect(checkbox?.disabled).toBe(true);
    expect(checkbox?.checked).toBe(true);
    expect(checkbox?.hasAttribute("onclick")).toBe(false);
    expect(clean("<p>a<input type=\"radio\"></p>").querySelector("input")).toBeNull();
    expect(clean("<p>a<input></p>").querySelector("input")).toBeNull();
    expect(clean("<p>a<script>alert(1)</script></p>").querySelector("script")).toBeNull();
    const image = clean("<p>a<img src=x onerror=\"alert(1)\"></p>");
    expect(image.querySelector("img")).toBeNull();
    expect(image.innerHTML).not.toContain("onerror");
    expect(clean("<p><a href=\"javascript:alert(1)\">link</a></p>").querySelector("a")?.getAttribute("href") ?? null)
      .toBeNull();
    expect(clean("<table><tbody><tr><td style=\"color:red\" align=\"center\">c</td></tr></tbody></table>").innerHTML)
      .toBe("<table><tbody><tr><td align=\"center\">c</td></tr></tbody></table>");
  });

  it("keeps safe links opening in a new browsing context and drops images", () => {
    const html = renderMarkdown("![a & label](https://example.invalid/a.png) [docs](https://example.invalid/docs)");
    expect(html).not.toContain("<img");
    expect(html).toContain("a &amp; label");
    const link = fragment(html).querySelector("a");
    expect(link?.getAttribute("href")).toBe("https://example.invalid/docs");
    expect(link?.getAttribute("target")).toBe("_blank");
    expect(link?.getAttribute("rel")).toBe("noopener noreferrer");
  });
});
