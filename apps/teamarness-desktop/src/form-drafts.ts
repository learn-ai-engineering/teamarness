/**
 * Keeps what the Person typed across a re-render of a Shell screen. A screen is rebuilt from
 * markup whenever its model changes, including when a notice hides itself, so a field the Person
 * changed or is in would otherwise lose its text and focus.
 */

export type Draft = Readonly<{ selector: string; value: string; focused: boolean; start: number | null; end: number | null }>;

/** Fields the Person changed or is in, by a selector that finds the same field after a render. */
export function captureDrafts(screen: HTMLElement): readonly Draft[] {
  const drafts: Draft[] = [];
  for (const field of screen.querySelectorAll<HTMLInputElement | HTMLTextAreaElement>("input, textarea")) {
    const selector = field.id ? `#${field.id}` : field.classList.contains("member-name-input") ? ".member-name-input" : null;
    if (selector === null || field.type === "checkbox" || field.type === "radio") continue;
    const focused = field.ownerDocument.activeElement === field;
    if (!focused && field.value === field.defaultValue) continue;
    drafts.push({ selector, value: field.value, focused, start: field.selectionStart, end: field.selectionEnd });
  }
  return drafts;
}

/** Puts captured drafts back, and lets dependent controls, such as the delete button, see them. */
export function restoreDrafts(screen: HTMLElement, drafts: readonly Draft[]): void {
  for (const draft of drafts) {
    const field = screen.querySelector<HTMLInputElement | HTMLTextAreaElement>(draft.selector);
    if (!field || field.disabled) continue;
    if (field.value !== draft.value) {
      field.value = draft.value;
      const EventType = field.ownerDocument.defaultView?.Event ?? Event;
      field.dispatchEvent(new EventType("input", { bubbles: true }));
    }
    if (draft.focused) {
      field.focus();
      if (draft.start !== null && draft.end !== null) field.setSelectionRange(draft.start, draft.end);
    }
  }
}
