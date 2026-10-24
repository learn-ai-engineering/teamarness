import { describe, expect, it } from "vitest";
import { DocumentEditor } from "./editor";

const DOC = "7".repeat(64);

describe("read-only DocumentEditor", () => {
  it("shows each newer full text and ignores an older one", () => {
    let announced = 0;
    const editor = new DocumentEditor({ documentId: DOC, revision: "1", editable: true, text: "# Plan\n" }, () => { announced += 1; });
    editor.receive({ documentId: DOC, revision: "2", editable: true, text: "# Plan\n\nmore\n" });
    expect(editor.value).toBe("# Plan\n\nmore\n");
    editor.receive({ documentId: DOC, revision: "1", editable: true, text: "# Plan\n" });
    expect(editor.value).toBe("# Plan\n\nmore\n");
    expect(editor.current.revision).toBe("2");
    editor.receive({ documentId: "8".repeat(64), revision: "9", editable: true, text: "other" });
    expect(editor.value).toBe("# Plan\n\nmore\n");
    expect(announced).toBe(1);
  });

  it("becomes editable when pending history arrives at the same revision", () => {
    const editor = new DocumentEditor({ documentId: DOC, revision: "0", editable: false, text: "" }, () => {});
    editor.receive({ documentId: DOC, revision: "0", editable: true, text: "body\n" });
    expect(editor.current.editable).toBe(true);
    expect(editor.value).toBe("body\n");
  });

  it("assembles UTF-8 text chunks only after a complete transfer", () => {
    const editor = new DocumentEditor({ documentId: DOC, revision: "1", editable: true, text: "old" }, () => {});
    editor.receive({ documentId: DOC, revision: "2", editable: true, transfer: "begin", chunks: 2 });
    expect(editor.assembling).toBe(true);
    editor.receive({ documentId: DOC, revision: "2", editable: true, transfer: "chunk", chunkIndex: 0, chunk: "😀 one " });
    expect(editor.value).toBe("old");
    editor.receive({ documentId: DOC, revision: "2", editable: true, transfer: "chunk", chunkIndex: 1, chunk: "two" });
    editor.receive({ documentId: DOC, revision: "2", editable: true, transfer: "end" });
    expect(editor.assembling).toBe(false);
    expect(editor.value).toBe("😀 one two");
  });

  it("requests a resync for an interrupted or begin-less transfer", () => {
    let resyncs = 0;
    const editor = new DocumentEditor({ documentId: DOC, revision: "1", editable: true, text: "old" }, () => {}, () => { resyncs += 1; });
    editor.receive({ documentId: DOC, revision: "2", editable: true, transfer: "chunk", chunkIndex: 0, chunk: "x" });
    expect(resyncs).toBe(1);
    editor.receive({ documentId: DOC, revision: "3", editable: true, transfer: "begin", chunks: 2 });
    editor.receive({ documentId: DOC, revision: "3", editable: true, transfer: "chunk", chunkIndex: 1, chunk: "y" });
    expect(resyncs).toBe(2);
    expect(editor.value).toBe("old");
  });

  it("hides the body on an identity conflict and ignores later text", () => {
    const editor = new DocumentEditor({ documentId: DOC, revision: "1", editable: true, text: "secret" }, () => {});
    editor.receive({ documentId: DOC, revision: "1", editable: false, reason: "identity_conflict", text: "" });
    editor.receive({ documentId: DOC, revision: "3", editable: true, text: "secret" });
    expect(editor.value).toBe("");
    expect(editor.current.reason).toBe("identity_conflict");
  });

  it("keeps the last good text with its diagnostic record after an import failure", () => {
    const editor = new DocumentEditor({ documentId: DOC, revision: "1", editable: true, text: "good\n" }, () => {});
    editor.receive({ documentId: DOC, revision: "1", editable: false, reason: "import_failure", diagnosticRecordId: "4".repeat(64), text: "good\n" });
    editor.receive({ documentId: DOC, revision: "2", editable: true, text: "newer\n" });
    expect(editor.value).toBe("good\n");
    expect(editor.current).toMatchObject({ editable: false, reason: "import_failure", diagnosticRecordId: "4".repeat(64) });
  });
});
