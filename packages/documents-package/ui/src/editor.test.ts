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

  it("replaces everything on a full-text event and clamps the caret", () => {
    const editor = new DocumentEditor(event({ revision: "1", text: "old text" }), async () => ({ revision: "2" }), () => {});
    const caret = editor.receive(event({ revision: "5", text: "new", editable: false }), { start: 7, end: 8 });
    expect(editor.value).toBe("new");
    expect(editor.current.editable).toBe(false);
    expect(caret).toEqual({ start: 3, end: 3 });
  });
});
