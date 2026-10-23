import { describe, expect, it } from "vitest";
import { applyOps, diffOps, transformCaret, utf16ToScalar, scalarToUtf16 } from "./editor";

describe("identity conflict quarantine", () => {
  it("hides an already-Saved body rather than retaining it as a draft", () => {
    const editor = new DocumentEditor({ documentId: "7".repeat(64), revision: "1", editable: true, text: "secret" }, () => new Promise(() => {}), () => {});
    editor.receive({ documentId: "7".repeat(64), revision: "1", editable: false, reason: "identity_conflict", text: "" }, null);
    expect(editor.value).toBe("");
    expect(editor.current.text).toBe("");
  });
  it("does not restore text or Saved from an in-flight ACK after quarantine", async () => {
    let acknowledge!: (value: { revision: string }) => void;
    const editor = new DocumentEditor({ documentId: "7".repeat(64), revision: "1", editable: true, text: "secret" },
      () => new Promise((resolve) => { acknowledge = resolve; }), () => {});
    editor.input("secret draft");
    editor.receive({ documentId: "7".repeat(64), revision: "1", editable: false, reason: "identity_conflict", text: "" }, null);
    acknowledge({ revision: "2" });
    await Promise.resolve();
    expect(editor.current.text).toBe("");
    expect(editor.current.reason).toBe("identity_conflict");
    expect(editor.current.sending).toBe(false);
    expect(editor.value).toBe("secret draft");
  });

  it("hides text and ignores stale full and chunk transfers while retaining an unsaved draft", () => {
    const editor = new DocumentEditor({ documentId: "7".repeat(64), revision: "1", editable: true, text: "secret" }, () => new Promise(() => {}), () => {});
    editor.input("secret draft");
    editor.receive({ documentId: "7".repeat(64), revision: "2", editable: true, transfer: "begin", chunks: 1 }, null);
    editor.receive({ documentId: "7".repeat(64), revision: "2", editable: false, reason: "identity_conflict", text: "" }, null);
    editor.receive({ documentId: "7".repeat(64), revision: "3", editable: true, text: "secret" }, null);
    editor.receive({ documentId: "7".repeat(64), revision: "3", editable: true, transfer: "chunk", chunkIndex: 0, chunk: "secret" }, null);
    editor.receive({ documentId: "7".repeat(64), revision: "3", editable: true, transfer: "end" }, null);
    expect(editor.current.text).toBe("");
    expect(editor.current.editable).toBe(false);
    expect(editor.current.reason).toBe("identity_conflict");
    expect(editor.value).toBe("secret draft");
  });
});

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

  it("rebases a near-budget ASCII and Unicode draft through delta and full recovery", () => {
    for (const draft of ["a".repeat(190 * 1024), "😀".repeat(45 * 1024)]) {
      expect(applyOps("Z", [{ retain: 1 }, { insert: draft }])).toBe(`Z${draft}`);
      const editor = new DocumentEditor(event({ revision: "0", text: "base" }), () => new Promise(() => {}), () => {});
      editor.input(`base${draft}`);
      editor.receive(event({ revision: "1", origin: "remote", delta: [{ insert: "Z" }] }), null);
      expect(editor.value).toBe(`Zbase${draft}`);
      editor.receive(recovered(editor, { revision: "2", text: "YZbase" }, "absent"), null);
      expect(editor.value).toBe(`YZbase${draft}`);
      editor.receive(recovered(editor, { revision: "3", transfer: "begin", chunks: 2 }, "absent"), null);
      editor.receive(event({ revision: "3", transferId, transfer: "chunk", chunkIndex: 0, chunk: "X" }), null);
      editor.receive(event({ revision: "3", transferId, transfer: "chunk", chunkIndex: 1, chunk: "YZbase" }), null);
      editor.receive(event({ revision: "3", transferId, transfer: "end" }), null);
      expect(editor.value).toBe(`XYZbase${draft}`);
    }
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

const transferId = "f".repeat(32);
function recovered(editor: DocumentEditor, partial: Partial<TextEvent> & { revision: string },
  inclusion: "included" | "absent" | "unresolved", durability: "durable" | "pending" | "unresolved" = "unresolved"): TextEvent {
  const id = editor.outstandingIds[0];
  if (id === undefined) return event(partial);
  editor.beginRecovery(transferId);
  return event({ ...partial, transferId, requestStatuses: [{ requestId: id, inclusion, durability }] });
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

  it("clears only the matching failed edit after a background durable append, preserving newer typing", async () => {
    const requests: { base: string; ops: unknown }[] = [];
    const editor = new DocumentEditor(event({ revision: "0", text: "" }),
      async (_id, base, ops) => { requests.push({ base, ops }); throw { code: "documents.capability_unavailable" }; }, () => {});
    editor.input("draft");
    await Promise.resolve();
    expect(editor.current.error).not.toBeNull();
    editor.receive(event({ revision: "1", origin: "local", delta: [{ insert: "draft" }] }), null);
    editor.input("draft more");
    await Promise.resolve();
    expect(editor.current.error).not.toBeNull();
    editor.receive(recovered(editor, { revision: "1", text: "draft", savedRevision: "99" }, "included", "pending"), null);
    expect(editor.current.error).not.toBeNull();
    editor.receive(recovered(editor, { revision: "1", text: "draft", savedRevision: "1" }, "included", "durable"), null);
    expect(editor.current.error).toBeNull();
    expect(editor.current.text).toBe("draft");
    expect(editor.value).toBe("draft more");
    expect(requests).toEqual([
      { base: "0", ops: [{ insert: "draft" }] },
      { base: "0", ops: [{ insert: "draft" }] },
      { base: "1", ops: [{ retain: 5 }, { insert: " more" }] },
    ]);
  });

  it("shows Saved after background completion with no further input, including chunked recovery", async () => {
    let sends = 0;
    const editor = new DocumentEditor(event({ revision: "0", text: "" }),
      async () => { sends++; throw { code: "documents.capability_unavailable" }; }, () => {});
    editor.input("draft");
    await Promise.resolve();
    expect(editor.current.error).not.toBeNull();
    editor.receive(recovered(editor, { revision: "1", transfer: "begin", chunks: 2 }, "included", "durable"), null);
    editor.receive(event({ revision: "1", transferId, transfer: "chunk", chunkIndex: 0, chunk: "dra" }), null);
    editor.receive(event({ revision: "1", transferId, transfer: "chunk", chunkIndex: 1, chunk: "ft" }), null);
    editor.receive(event({ revision: "1", transferId, transfer: "end", savedRevision: "1" }), null);
    expect(editor.current.error).toBeNull();
    expect(editor.current.sending).toBe(false);
    expect(editor.current.text).toBe("draft");
    expect(editor.value).toBe("draft");
    expect(sends).toBe(1);
  });

  it("reconciles a durable event preceding a failed request response", async () => {
    let rejectSend: (cause: unknown) => void = () => {};
    const editor = new DocumentEditor(event({ revision: "0", text: "" }),
      () => new Promise((_resolve, reject) => { rejectSend = reject; }), () => {});
    editor.input("draft");
    editor.receive(recovered(editor, { revision: "1", text: "draft", savedRevision: "1" }, "included", "durable"), null);
    rejectSend({ code: "documents.capability_unavailable" });
    await Promise.resolve();
    expect(editor.current.error).toBeNull();
    expect(editor.current.text).toBe("draft");
    expect(editor.current.sending).toBe(false);
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
    editor.receive(recovered(editor, { revision: "2", text: "Zabc" }, "absent"), null);
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
    editor.receive(event({ revision: "1", origin: "local", requestId: editor.outstandingIds[0], delta: [{ retain: 3 }, { insert: "X" }] }), null);
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
    editor.receive(recovered(editor, { revision: "2", text: "Zabc" }, "absent"), null);
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
    editor.receive(recovered(editor, { revision: "2", text: "ZabcX" }, "included", "pending"), null);
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
    editor.receive(recovered(editor, { revision: "2", transfer: "begin", chunks: 2 }, "included", "pending"), null);
    editor.receive(event({ revision: "2", transferId, transfer: "chunk", chunkIndex: 0, chunk: "Zabc" }), null);
    editor.receive(event({ revision: "2", transferId, transfer: "chunk", chunkIndex: 1, chunk: "X" }), null);
    editor.receive(event({ revision: "2", transferId, transfer: "end" }), null);
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
    editor.receive(recovered(editor, { revision: "2", text: "Zabc" }, "absent"), null);
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
    editor.receive(recovered(editor, { revision: "2", transfer: "begin", chunks: 1 }, "absent"), null);
    editor.receive(event({ revision: "1", transferId, transfer: "chunk", chunkIndex: 0, chunk: "old" }), null);
    editor.receive(event({ revision: "2", transferId, transfer: "chunk", chunkIndex: 0, chunk: "za" }), null);
    editor.receive(event({ revision: "2", transferId, transfer: "end" }), null);
    expect(editor.current.text).toBe("za");
    expect(editor.value).toBe("zaX");
  });

  it("does not duplicate an in-flight insertion interleaved with remote text in full or chunked recovery", async () => {
    for (const chunked of [false, true]) {
      for (const acknowledgeFirst of [false, true]) {
        let acknowledge: (value: { revision: string }) => void = () => {};
        const editor = new DocumentEditor(event({ revision: "0", text: "abc" }),
          () => new Promise((resolve) => { acknowledge = resolve; }), () => {});
        editor.input("abcX");
        editor.input("abcXY");
        if (acknowledgeFirst) {
          acknowledge({ revision: "1" });
          await Promise.resolve();
        }
        if (chunked) {
          editor.receive(recovered(editor, { revision: "2", transfer: "begin", chunks: 2 }, acknowledgeFirst ? "absent" : "included", acknowledgeFirst ? "unresolved" : "durable"), null);
          const token = editor.outstandingIds.length > 0 ? transferId : undefined;
          editor.receive(event({ revision: "2", transferId: token, transfer: "chunk", chunkIndex: 0, chunk: "abcZ" }), null);
          editor.receive(event({ revision: "2", transferId: token, transfer: "chunk", chunkIndex: 1, chunk: "X" }), null);
          editor.receive(event({ revision: "2", transferId: token, transfer: "end", savedRevision: "1" }), null);
        } else {
          editor.receive(recovered(editor, { revision: "2", text: "abcZX", savedRevision: "1" }, acknowledgeFirst ? "absent" : "included", acknowledgeFirst ? "unresolved" : "durable"), null);
        }
        expect(editor.current.text).toBe("abcZX");
        expect(editor.value).toBe("abcZXY");
        if (!acknowledgeFirst) {
          acknowledge({ revision: "1" });
          await Promise.resolve();
          expect(editor.value).toBe("abcZXY");
        }
      }
    }
  });

  it("sends a valid shrink after terminal document_too_large rather than replaying rejected growth", async () => {
    const limit = "a".repeat(192 * 1024);
    const requests: { base: string; ops: unknown }[] = [];
    const editor = new DocumentEditor(event({ revision: "4", text: limit }),
      async (_id, base, ops) => {
        requests.push({ base, ops });
        if (requests.length === 1) throw { code: "documents.document_too_large" };
        return { revision: "5" };
      }, () => {});
    editor.input(`${limit}X`);
    await Promise.resolve();
    expect(editor.current.error).toBe("This document has reached its size limit.");
    expect(editor.current.text).toBe(limit);
    editor.input(limit.slice(0, -1));
    await Promise.resolve();
    expect(requests).toEqual([
      { base: "4", ops: [{ retain: limit.length }, { insert: "X" }] },
      { base: "4", ops: [{ retain: limit.length - 1 }, { delete: 1 }] },
    ]);
    expect(editor.current.error).toBeNull();
    expect(editor.current.text).toBe(limit.slice(0, -1));
  });

  it("preserves a remote insertion at a local deletion boundary before ACK and on full recovery", async () => {
    for (const full of [false, true]) {
      let acknowledge: (value: { revision: string }) => void = () => {};
      const editor = new DocumentEditor(event({ revision: "0", text: "a😀bc" }),
        () => new Promise((resolve) => { acknowledge = resolve; }), () => {});
      editor.input("a😀c");
      const caret = editor.receive(full
        ? recovered(editor, { revision: "1", text: "a😀bXc" }, "absent")
        : event({ revision: "1", origin: "remote", delta: [{ retain: 3 }, { insert: "X" }] }),
      { start: 3, end: 3 });
      expect(editor.current.text).toBe("a😀bXc");
      expect(editor.value).toBe("a😀Xc");
      expect(caret).toEqual({ start: 3, end: 3 });
      acknowledge({ revision: "1" });
      await Promise.resolve();
      expect(editor.value).toBe("a😀Xc");
    }
  });

  it("keeps remote insertions before, inside, and at either edge of a deleted span", async () => {
    for (const [delta, authoritative, draft] of [
      [[{ insert: "X" }], "Xabcd", "Xad"],
      [[{ retain: 1 }, { insert: "X" }], "aXbcd", "aXd"],
      [[{ retain: 2 }, { insert: "X" }], "abXcd", "aXd"],
      [[{ retain: 3 }, { insert: "X" }], "abcXd", "aXd"],
    ] as const) {
      const editor = new DocumentEditor(event({ revision: "0", text: "abcd" }), () => new Promise(() => {}), () => {});
      editor.input("ad");
      editor.receive(event({ revision: "1", origin: "remote", delta: [...delta] }), null);
      expect(editor.current.text).toBe(authoritative);
      expect(editor.value).toBe(draft);
    }
  });

  it("splits a large Unicode action into sequential separately identified requests", async () => {
    const calls: { id: string; base: string; text: string }[] = [];
    const editor = new DocumentEditor(event({ revision: "0", text: "" }),
      async (_document, base, ops, id) => {
        calls.push({ id, base, text: applyOps("", ops) });
        return { revision: String(calls.length) };
      }, () => {});
    editor.input("😀".repeat(5000));
    for (let index = 0; index < 20; index++) await Promise.resolve();
    expect(calls.map((call) => Array.from(call.text).length)).toEqual([2048, 2048, 904]);
    expect(new Set(calls.map((call) => call.id)).size).toBe(3);
    expect(calls.map((call) => call.base)).toEqual(["0", "1", "2"]);
    expect(editor.value).toBe("😀".repeat(5000));
    expect(editor.current.text).toBe(editor.value);
  });

  it("re-splits a measured oversized record with a fresh ID without changing the draft", async () => {
    const requests: { id: string; length: number }[] = [];
    const editor = new DocumentEditor(event({ revision: "0", text: "" }), async (_document, _base, ops, id) => {
      const last = ops.at(-1);
      requests.push({ id, length: last && "insert" in last ? last.insert.length : 0 });
      if (requests.length === 1) throw { code: "documents.edit_request_too_large" };
      return { revision: String(requests.length - 1) };
    }, () => {});
    editor.input("x".repeat(12000));
    for (let index = 0; index < 20; index++) await Promise.resolve();
    expect(requests.map((request) => request.length)).toEqual([8192, 4096, 4096, 3808]);
    expect(new Set(requests.map((request) => request.id)).size).toBe(4);
    expect(editor.value).toBe("x".repeat(12000));
    expect(editor.current.text).toBe(editor.value);
  });

  it("durable but unprojected recovery keeps the outstanding ID and unsaved draft", async () => {
    let sends = 0;
    const editor = new DocumentEditor(event({ revision: "0", text: "good" }),
      () => { sends++; return new Promise(() => {}); }, () => {});
    editor.input("good draft");
    const id = editor.outstandingIds[0]!;
    editor.receive(recovered(editor, { revision: "1", text: "good draft", savedRevision: "1" },
      "unresolved", "durable"), null);
    expect(editor.outstandingIds).toEqual([id]);
    expect(editor.value).toBe("good draft");
    expect(editor.current.error).not.toBeNull();
    expect(sends).toBe(1);
    editor.receive(recovered(editor, { revision: "1", text: "good draft", savedRevision: "1" },
      "included", "durable"), null);
    expect(editor.outstandingIds).toEqual([]);
  });

  it("unresolved identified recovery keeps a draft unsaved and never resends guessed bytes", async () => {
    let sends = 0;
    const editor = new DocumentEditor(event({ revision: "0", text: "abc" }),
      () => { sends++; return new Promise(() => {}); }, () => {});
    editor.input("abcX");
    editor.receive(recovered(editor, { revision: "1", text: "abc" }, "unresolved"), null);
    expect(editor.value).toBe("abcX");
    expect(editor.current.error).not.toBeNull();
    editor.input("abcXY");
    await Promise.resolve();
    expect(sends).toBe(1);
  });

  it("restores the applied baseline after a coalescing preview fails without erasing the draft", () => {
    const editor = new DocumentEditor(event({ revision: "1", text: "good" }), () => new Promise(() => {}), () => {});
    editor.input("good draft");
    const id = editor.outstandingIds[0]!;
    editor.receive(event({ revision: "2", origin: "local", requestId: id,
      delta: [{ retain: 4 }, { insert: " draft" }] }), null);
    editor.receive(event({ revision: "2", text: "good", editable: false, reason: "import_failure",
      diagnosticRecordId: "b".repeat(64) }), null);
    expect(editor.current.text).toBe("good");
    expect(editor.value).toBe("good draft");
    editor.beginRecovery("a".repeat(32));
    editor.receive(event({ revision: "2", text: "good", editable: false, reason: "import_failure",
      diagnosticRecordId: "b".repeat(64), transferId: "a".repeat(32),
      requestStatuses: [{ requestId: id, inclusion: "unresolved", durability: "unresolved" }] }), null);
    expect(editor.current.text).toBe("good");
    expect(editor.value).toBe("good draft");
    expect(editor.outstandingIds).toEqual([id]);
    expect(editor.current.editable).toBe(false);
  });

  it("keeps an outstanding draft on import failure even if the sealed edit is durable", () => {
    const editor = new DocumentEditor(event({ revision: "0", text: "good" }), () => new Promise(() => {}), () => {});
    editor.input("good draft");
    const id = editor.outstandingIds[0]!;
    editor.beginRecovery("a".repeat(32));
    editor.receive(event({ revision: "0", text: "good", editable: false,
      reason: "import_failure", diagnosticRecordId: "b".repeat(64), transferId: "a".repeat(32),
      requestStatuses: [{ requestId: id, inclusion: "unresolved", durability: "durable" }] }), null);
    expect(editor.current.reason).toBe("import_failure");
    expect(editor.current.diagnosticRecordId).toBe("b".repeat(64));
    expect(editor.value).toBe("good draft");
    expect(editor.outstandingIds).toEqual([id]);
    expect(editor.current.editable).toBe(false);
  });

  it("replaces everything on a full-text event and clamps the caret", () => {
    const editor = new DocumentEditor(event({ revision: "1", text: "old text" }), async () => ({ revision: "2" }), () => {});
    const caret = editor.receive(event({ revision: "5", text: "new", editable: false }), { start: 7, end: 8 });
    expect(editor.value).toBe("new");
    expect(editor.current.editable).toBe(false);
    expect(caret).toEqual({ start: 3, end: 3 });
  });
});
