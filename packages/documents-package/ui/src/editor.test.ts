import { describe, expect, it } from "vitest";
import { applyOps, diffOps, transformCaret, utf16ToScalar, scalarToUtf16 } from "./editor";

describe("text ops", () => {
  it("diffs to a minimal retain/delete/insert and applies back", () => {
    for (const [before, after] of [
      ["", "hello"], ["hello", ""], ["abc", "abXc"], ["abc", "ac"], ["héllo", "hállo"],
      ["aaa", "aa"], ["# Plan\n", "# Plan\n- item\n"], ["x😀y", "xy"], ["same", "same"],
    ] as const) {
      const ops = diffOps(before, after);
      expect(applyOps(before, ops)).toBe(after);
      if (before === after) expect(ops).toEqual([]);
    }
    expect(diffOps("abc", "abXc")).toEqual([{ retain: 2 }, { insert: "X" }]);
    expect(diffOps("abc", "ac")).toEqual([{ retain: 1 }, { delete: 1 }]);
  });

  it("moves a caret across remote inserts and deletes", () => {
    expect(transformCaret(5, [{ retain: 2 }, { insert: "ab" }])).toBe(7);
    expect(transformCaret(1, [{ retain: 2 }, { insert: "ab" }])).toBe(1);
    expect(transformCaret(5, [{ retain: 2 }, { delete: 2 }])).toBe(3);
    expect(transformCaret(3, [{ retain: 2 }, { delete: 5 }])).toBe(2);
    expect(transformCaret(0, [{ insert: "x" }])).toBe(1);
  });

  it("converts between UTF-16 offsets and scalar positions", () => {
    const text = "a😀b";
    expect(utf16ToScalar(text, 3)).toBe(2);
    expect(scalarToUtf16(text, 2)).toBe(3);
    expect(scalarToUtf16(text, 3)).toBe(4);
  });
});

import { DocumentEditor } from "./editor";
import type { TextEvent } from "./contract.generated";

const doc = "7".repeat(64);
function event(partial: Partial<TextEvent> & { revision: string }): TextEvent {
  return { documentId: doc, editable: true, ...partial };
}

describe("DocumentEditor", () => {
  it("sends one edit at a time against the acknowledged revision and echoes keep the draft", async () => {
    const sent: { base: string; ops: unknown }[] = [];
    let resolveSend: (value: { revision: string }) => void = () => {};
    const editor = new DocumentEditor(
      event({ revision: "0", text: "" }),
      (_id, base, ops) => {
        sent.push({ base, ops });
        return new Promise((resolve) => { resolveSend = resolve; });
      },
      () => {},
    );
    editor.input("he");
    editor.input("hel");
    expect(sent).toEqual([{ base: "0", ops: [{ insert: "he" }] }]);
    expect(editor.current.sending).toBe(true);
    resolveSend({ revision: "1" });
    await Promise.resolve();
    await Promise.resolve();
    expect(sent.at(-1)).toEqual({ base: "1", ops: [{ retain: 2 }, { insert: "l" }] });
    editor.receive(event({ revision: "1", origin: "local", delta: [{ insert: "he" }] }), null);
    expect(editor.value).toBe("hel");
  });

  it("rebases an unsent draft and the caret over a remote delta", () => {
    const editor = new DocumentEditor(event({ revision: "3", text: "abc" }), () => new Promise(() => {}), () => {});
    editor.input("abcX");
    // The send is in flight (never resolves here); a remote insert lands at the front.
    const caret = editor.receive(event({ revision: "4", origin: "remote", delta: [{ insert: "ZZ" }] }), { start: 4, end: 4 });
    expect(editor.current.text).toBe("ZZabc");
    expect(editor.value).toBe("ZZabcX");
    expect(caret).toEqual({ start: 6, end: 6 });
  });

  it("keeps failed edits unsaved and retries the identical request", async () => {
    const requests: { base: string; ops: unknown }[] = [];
    let fail = true;
    const editor = new DocumentEditor(event({ revision: "0", text: "" }),
      async (_id, base, ops) => {
        requests.push({ base, ops });
        if (fail) throw { code: "documents.capability_unavailable" };
        return { revision: "1" };
      }, () => {});
    editor.input("draft");
    await Promise.resolve();
    expect(editor.value).toBe("draft");
    expect(editor.current.error).not.toBeNull();
    fail = false;
    editor.input("draft");
    await Promise.resolve();
    expect(requests).toEqual([
      { base: "0", ops: [{ insert: "draft" }] },
      { base: "0", ops: [{ insert: "draft" }] },
    ]);
    expect(editor.current.error).toBeNull();
    expect(editor.current.text).toBe("draft");
  });

  it("does not acknowledge typing added after a failed request as saved on retry", async () => {
    let acknowledge: (value: { revision: string }) => void = () => {};
    let attempts = 0;
    const editor = new DocumentEditor(event({ revision: "0", text: "" }),
      () => {
        attempts += 1;
        if (attempts === 1) return Promise.reject({ code: "documents.capability_unavailable" });
        return new Promise((resolve) => { acknowledge = resolve; });
      }, () => {});
    editor.input("draft");
    await Promise.resolve();
    editor.input("draft more");
    acknowledge({ revision: "1" });
    await Promise.resolve();
    expect(editor.current.text).toBe("draft");
    expect(editor.value).toBe("draft more");
  });

  it("does not replace a newer remote revision when an old ACK arrives", async () => {
    let acknowledge: (value: { revision: string }) => void = () => {};
    const editor = new DocumentEditor(event({ revision: "0", text: "abc" }),
      () => new Promise((resolve) => { acknowledge = resolve; }), () => {});
    editor.input("abcX");
    editor.receive(event({ revision: "2", text: "Zabc" }), null);
    acknowledge({ revision: "1" });
    await Promise.resolve();
    expect(editor.current.revision).toBe("2");
    expect(editor.value).toBe("ZabcX");
    expect(editor.current.text).toBe("Zabc");
  });

  it("preserves the remote change when a local echo and remote delta precede ACK", async () => {
    let acknowledge: (value: { revision: string }) => void = () => {};
    const editor = new DocumentEditor(event({ revision: "0", text: "abc" }),
      () => new Promise((resolve) => { acknowledge = resolve; }), () => {});
    editor.input("abcX");
    editor.receive(event({ revision: "1", origin: "local", delta: [{ retain: 3 }, { insert: "X" }] }), null);
    editor.receive(event({ revision: "2", origin: "remote", delta: [{ insert: "Z" }] }), null);
    acknowledge({ revision: "1" });
    await Promise.resolve();
    expect(editor.current.text).toBe("ZabcX");
    expect(editor.value).toBe("ZabcX");
    expect(editor.current.revision).toBe("2");
  });

  it("rebases both an in-flight edit and an unsent draft on full recovery", async () => {
    let acknowledge: (value: { revision: string }) => void = () => {};
    const editor = new DocumentEditor(event({ revision: "0", text: "abc" }),
      () => new Promise((resolve) => { acknowledge = resolve; }), () => {});
    editor.input("abcX");
    editor.input("abcXY");
    editor.receive(event({ revision: "2", text: "Zabc" }), null);
    expect(editor.value).toBe("ZabcXY");
    acknowledge({ revision: "1" });
    await Promise.resolve();
    expect(editor.current.revision).toBe("2");
    expect(editor.value).toBe("ZabcXY");
  });

  it("does not repeat an in-flight insert already present in a full recovery", async () => {
    let acknowledge: (value: { revision: string }) => void = () => {};
    const editor = new DocumentEditor(event({ revision: "0", text: "abc" }),
      () => new Promise((resolve) => { acknowledge = resolve; }), () => {});
    editor.input("abcX");
    editor.input("abcXY");
    editor.receive(event({ revision: "2", text: "ZabcX" }), null);
    expect(editor.value).toBe("ZabcXY");
    acknowledge({ revision: "1" });
    await Promise.resolve();
    expect(editor.value).toBe("ZabcXY");
  });

  it("assembles UTF-8 text chunks only after a complete revision and preserves an unsent draft", () => {
    const editor = new DocumentEditor(event({ revision: "0", text: "abc", editable: false }), () => new Promise(() => {}), () => {});
    editor.input("abcX");
    editor.receive(event({ revision: "1", transfer: "begin", chunks: 2 }), null);
    editor.receive(event({ revision: "1", transfer: "chunk", chunkIndex: 0, chunk: "Zabc" }), null);
    expect(editor.current.text).toBe("abc");
    editor.receive(event({ revision: "1", transfer: "chunk", chunkIndex: 1, chunk: "😀" }), null);
    editor.receive(event({ revision: "1", transfer: "end" }), null);
    expect(editor.current.text).toBe("Zabc😀");
    expect(editor.value).toBe("Zabc😀X");
  });

  it("rebases an in-flight edit and later typing over a chunked recovery", async () => {
    let acknowledge: (value: { revision: string }) => void = () => {};
    const editor = new DocumentEditor(event({ revision: "0", text: "abc" }),
      () => new Promise((resolve) => { acknowledge = resolve; }), () => {});
    editor.input("abcX");
    editor.input("abcXY");
    editor.receive(event({ revision: "2", transfer: "begin", chunks: 2 }), null);
    editor.receive(event({ revision: "2", transfer: "chunk", chunkIndex: 0, chunk: "Zabc" }), null);
    editor.receive(event({ revision: "2", transfer: "chunk", chunkIndex: 1, chunk: "X" }), null);
    editor.receive(event({ revision: "2", transfer: "end" }), null);
    expect(editor.value).toBe("ZabcXY");
    acknowledge({ revision: "1" });
    await Promise.resolve();
    expect(editor.current.revision).toBe("2");
    expect(editor.value).toBe("ZabcXY");
  });

  it("requests full resync on a delta revision gap and retains unsent typing", () => {
    let resyncs = 0;
    const editor = new DocumentEditor(event({ revision: "0", text: "abc", editable: false }),
      () => new Promise(() => {}), () => {}, () => { resyncs++; });
    editor.input("abcX");
    editor.receive(event({ revision: "2", origin: "remote", delta: [{ insert: "Z" }] }), null);
    expect(resyncs).toBe(1);
    expect(editor.current.text).toBe("abc");
    expect(editor.value).toBe("abcX");
    editor.receive(event({ revision: "2", text: "Zabc" }), null);
    expect(editor.value).toBe("ZabcX");
  });

  it("requests resync when a begin frame is missing", () => {
    let resyncs = 0;
    const editor = new DocumentEditor(event({ revision: "0", text: "a", editable: false }),
      () => new Promise(() => {}), () => {}, () => { resyncs++; });
    editor.input("aX");
    editor.receive(event({ revision: "1", transfer: "chunk", chunkIndex: 0, chunk: "z" }), null);
    editor.receive(event({ revision: "1", transfer: "end" }), null);
    expect(resyncs).toBe(1);
    expect(editor.value).toBe("aX");
  });

  it("discards stale/interrupted chunks and resynchronizes on a missing chunk without losing the draft", () => {
    let resyncs = 0;
    const editor = new DocumentEditor(event({ revision: "0", text: "a" }), () => new Promise(() => {}), () => {}, () => { resyncs++; });
    editor.input("aX");
    editor.receive(event({ revision: "1", transfer: "begin", chunks: 2 }), null);
    editor.receive(event({ revision: "1", transfer: "chunk", chunkIndex: 1, chunk: "z" }), null);
    expect(resyncs).toBe(1);
    editor.receive(event({ revision: "1", transfer: "end" }), null);
    editor.receive(event({ revision: "2", transfer: "begin", chunks: 1 }), null);
    editor.receive(event({ revision: "1", transfer: "chunk", chunkIndex: 0, chunk: "old" }), null);
    editor.receive(event({ revision: "2", transfer: "chunk", chunkIndex: 0, chunk: "za" }), null);
    editor.receive(event({ revision: "2", transfer: "end" }), null);
    expect(editor.current.text).toBe("za");
    expect(editor.value).toBe("zaX");
  });

  it("replaces everything on a full-text event and clamps the caret", () => {
    const editor = new DocumentEditor(event({ revision: "1", text: "old text" }), async () => ({ revision: "2" }), () => {});
    const caret = editor.receive(event({ revision: "5", text: "new", editable: false }), { start: 7, end: 8 });
    expect(editor.value).toBe("new");
    expect(editor.current.editable).toBe(false);
    expect(caret).toEqual({ start: 3, end: 3 });
  });
});
