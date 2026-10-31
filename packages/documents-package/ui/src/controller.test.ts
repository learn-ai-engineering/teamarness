/// <reference types="node" />
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { createFakePackageClient } from "@teamarness/package-ui-sdk/testing";
import { describe, expect, it } from "vitest";
import { DocumentsController, MARKDOWN_READ_ATTEMPTS, readChunkedMarkdown } from "./controller";

const DOCUMENT = "d".repeat(64);

function digest(text: string): string {
  return createHash("sha256").update(text, "utf8").digest("hex");
}

/** A Backend export of `text` cut into `size`-character chunks, as `documents.document.markdown` answers. */
function chunked(text: string, size: number) {
  const parts: string[] = [];
  for (let start = 0; start < text.length; start += size) parts.push(text.slice(start, start + size));
  if (parts.length === 0) parts.push("");
  return (chunkIndex: number) => ({
    documentId: DOCUMENT, digest: digest(text), chunks: parts.length, chunkIndex, chunk: parts[chunkIndex] ?? "",
  });
}

describe("copy as Markdown", () => {
  it("reassembles every chunk in order", async () => {
    const text = "# Title\n\n" + "Paragraph é ✓ text.\n\n".repeat(400);
    const read = chunked(text, 1000);
    const asked: number[] = [];
    const result = await readChunkedMarkdown(async (index) => { asked.push(index); return read(index); });
    expect(result).toBe(text);
    expect(asked).toEqual(Array.from({ length: Math.ceil(text.length / 1000) }, (_, index) => index));
  });

  it("reads an empty Document as one empty chunk", async () => {
    const read = chunked("", 10);
    await expect(readChunkedMarkdown(async (index) => read(index))).resolves.toBe("");
  });

  it("starts over from chunk 0 when the Document changes during the read", async () => {
    const before = chunked("a".repeat(30), 10);
    const after = chunked("b".repeat(25), 10);
    let calls = 0;
    const asked: number[] = [];
    const result = await readChunkedMarkdown(async (index) => {
      asked.push(index);
      calls += 1;
      return calls <= 2 ? before(index) : after(index);
    });
    expect(result).toBe("b".repeat(25));
    expect(asked).toEqual([0, 1, 2, 0, 1, 2]);
  });

  it("refuses text that does not match its digest", async () => {
    const read = chunked("right", 10);
    await expect(readChunkedMarkdown(async (index) => ({ ...read(index), chunk: "wrong" })))
      .rejects.toThrow(/kept changing/);
  });

  it("gives up when the Document keeps changing", async () => {
    let version = 0;
    let calls = 0;
    await expect(readChunkedMarkdown(async (index) => {
      calls += 1;
      version += 1;
      return chunked(`${"v".repeat(20)}${version}`, 10)(index);
    })).rejects.toThrow(/kept changing/);
    expect(calls).toBeLessThanOrEqual(MARKDOWN_READ_ATTEMPTS * 3);
  });
});

const GENESIS = new Uint8Array(readFileSync(join(import.meta.dirname, "../../fixtures/body-trees/from-rust/00-genesis.loro")));

function tick(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

describe("switching Documents", () => {
  it("shows the closed-lost notice once the next Document has opened", async () => {
    const first = "4".repeat(64);
    const second = "5".repeat(64);
    const fake = createFakePackageClient({
      requests: {
        "documents.comments": () => ({ comments: [] }),
        // The Backend never acknowledges the update, so it is still outstanding at the switch.
        "documents.document.update": () => new Promise<never>(() => undefined),
      },
      subscriptions: { "documents.document.session": () => undefined },
    });
    const controller = new DocumentsController(fake.client, () => undefined);
    await controller.openDocumentById(first);
    await fake.emit("documents.document.session", {
      event: "begin", documentId: first, sessionPeer: "7", version: "0", chunks: 1, editable: true,
    });
    await fake.emit("documents.document.session", { event: "chunk", documentId: first, chunkIndex: 0, chunk: Buffer.from(GENESIS).toString("base64") });
    await fake.emit("documents.document.session", { event: "end", documentId: first });
    const session = controller.editorSession!;
    const doc = session.doc!;
    doc.getText("typed").insert(0, "unconfirmed");
    doc.commit();
    session.flush();
    await tick();
    expect(session.state.outstanding).toBe(1);

    await controller.openDocumentById(second);
    expect(controller.state.openDocumentId).toBe(second);
    expect(controller.state.error).toMatch(/could not be confirmed as saved/);
  }, 10_000);
});
