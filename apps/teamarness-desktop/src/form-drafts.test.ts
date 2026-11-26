import { describe, expect, it } from "vitest";
import { captureDrafts, restoreDrafts } from "./form-drafts";

type FakeDocument = { activeElement: unknown; defaultView: null };

class FakeField {
  public selectionStart: number | null = null;
  public selectionEnd: number | null = null;
  public disabled = false;
  public readonly events: string[] = [];
  public readonly classList: { contains(name: string): boolean };

  public constructor(
    public readonly ownerDocument: FakeDocument,
    public readonly id: string,
    public value: string,
    public readonly defaultValue: string,
    public readonly type = "text",
    className = "",
  ) {
    this.classList = { contains: (name) => className.split(" ").includes(name) };
  }

  public focus(): void { this.ownerDocument.activeElement = this; }
  public setSelectionRange(start: number, end: number): void { this.selectionStart = start; this.selectionEnd = end; }
  public dispatchEvent(event: Event): boolean { this.events.push(event.type); return true; }
}

/** A screen of fields, found by `#id` or by class, like the Shell's rendered markup. */
function screen(fields: FakeField[]): HTMLElement {
  return {
    querySelectorAll: () => fields,
    querySelector: (selector: string) => fields.find((field) =>
      selector.startsWith("#") ? `#${field.id}` === selector : field.classList.contains(selector.slice(1))) ?? null,
  } as unknown as HTMLElement;
}

describe("form drafts across a re-render", () => {
  it("keeps changed and focused fields, and only those", () => {
    const document: FakeDocument = { activeElement: null, defaultView: null };
    const typed = new FakeField(document, "delete-workspace-confirm", "Throwa", "");
    const untouched = new FakeField(document, "rename-workspace-name", "Team", "Team");
    const focused = new FakeField(document, "agent-name", "", "");
    const checkbox = new FakeField(document, "flag", "on", "", "checkbox");
    const unnamed = new FakeField(document, "", "typed", "");
    const agent = new FakeField(document, "", "Helper 2", "Helper", "text", "member-name-input");
    focused.focus();
    focused.selectionStart = 0;
    focused.selectionEnd = 0;

    const drafts = captureDrafts(screen([typed, untouched, focused, checkbox, unnamed, agent]));
    expect(drafts.map(({ selector }) => selector)).toEqual(["#delete-workspace-confirm", "#agent-name", ".member-name-input"]);

    // The render replaced every field with a fresh one showing the model's value.
    document.activeElement = null;
    const fresh = [
      new FakeField(document, "delete-workspace-confirm", "", ""),
      new FakeField(document, "rename-workspace-name", "Team", "Team"),
      new FakeField(document, "agent-name", "", ""),
      new FakeField(document, "", "Helper", "Helper", "text", "member-name-input"),
    ];
    restoreDrafts(screen(fresh), drafts);
    expect(fresh.map(({ value }) => value)).toEqual(["Throwa", "Team", "", "Helper 2"]);
    // A restored value tells its listeners, so the delete button re-checks the typed name.
    expect(fresh[0]!.events).toEqual(["input"]);
    expect(fresh[2]!.events).toEqual([]);
    expect(document.activeElement).toBe(fresh[2]);
    expect([fresh[2]!.selectionStart, fresh[2]!.selectionEnd]).toEqual([0, 0]);
  });

  it("skips a field that is gone or disabled after the render", () => {
    const document: FakeDocument = { activeElement: null, defaultView: null };
    const drafts = captureDrafts(screen([new FakeField(document, "workspace-name", "New", ""), new FakeField(document, "agent-name", "Bot", "")]));
    const disabled = new FakeField(document, "agent-name", "", "");
    disabled.disabled = true;
    restoreDrafts(screen([disabled]), drafts);
    expect(disabled.value).toBe("");
  });
});
