import { describe, expect, it } from "vitest";
import { authorLabel, renderMarkdown, shortenedIdentity } from "./markdown";

describe("safe Conversations Markdown", () => {
  it("renders the supported formatting while escaping raw HTML", () => {
    const html = renderMarkdown("# Heading\n\n**bold** and *emphasis*\n\n<script>alert(1)</script>");
    expect(html).toContain("<h1>Heading</h1>");
    expect(html).toContain("<strong>bold</strong>");
    expect(html).toContain("<em>emphasis</em>");
    expect(html).not.toContain("<script>");
    expect(html).toContain("&lt;script&gt;");
  });

  it("turns images into escaped alt text and preserves safe link attributes", () => {
    const html = renderMarkdown("![a & label](https://example.invalid/a.png) [docs](https://example.invalid/docs)");
    expect(html).not.toContain("<img");
    expect(html).toContain("a &amp; label");
    expect(html).toContain('target="_blank"');
    expect(html).toContain('rel="noopener noreferrer"');
  });

  it("rejects unsafe and encoded URL schemes", () => {
    const html = renderMarkdown("[bad](javascript:alert(1)) [also bad](%6a%61vascript:alert(1))");
    expect(html).not.toContain("javascript:");
    expect(html).not.toContain("href=\"javascript");
  });
});

describe("identity presentation", () => {
  it("shows local authors as You and shortens remote Public Identities", () => {
    expect(authorLabel("alice-public-identity", "alice-public-identity")).toBe("You");
    expect(authorLabel("1234567890abcdef1234")).toBe("12345678…ef1234");
    expect(shortenedIdentity("short-key")).toBe("short-key");
  });
});
