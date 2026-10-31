import type { Priority, Status, Ticket } from "./contract.generated";
import { MAX_TITLE, PRIORITIES, PRIORITY_LABEL, STATUSES, STATUS_LABEL, UNASSIGNED } from "./controller";
import type { AssigneeOption, BacklogController, TicketFields } from "./controller";
import { Listbox } from "./listbox";
import { renderMarkdown } from "./markdown";

const KIT = {
  primary: "teamarness-control teamarness-control--primary",
  secondary: "teamarness-control teamarness-control--secondary",
  field: "teamarness-field",
  input: "teamarness-input",
  notice: "teamarness-notice",
} as const;

const timeFormat = new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" });

function element<K extends keyof HTMLElementTagNameMap>(tag: K, className: string, text?: string): HTMLElementTagNameMap[K] {
  const created = document.createElement(tag);
  created.className = className;
  if (text !== undefined) created.textContent = text;
  return created;
}

function button(label: string, className: string, onClick: () => void): HTMLButtonElement {
  const control = element("button", className, label);
  control.type = "button";
  control.addEventListener("click", onClick);
  return control;
}

/** Fills `line` with "<lead> <who> · <time>", the time in a `<time>` element. */
function attribution(line: HTMLElement, lead: string, who: string, at: number): void {
  const time = document.createElement("time");
  time.dateTime = new Date(at).toISOString();
  time.textContent = timeFormat.format(at);
  line.replaceChildren(`${lead} ${who} · `, time);
}

function field(label: string, control: HTMLElement): HTMLElement {
  const wrapper = element("div", `${KIT.field} ticket-field`);
  wrapper.append(element("span", "ticket-field-label", label), control);
  return wrapper;
}

/**
 * The selected Ticket: its title, status, priority, assignee, attribution and rendered body, and
 * the edit form that replaces the title and body while a Person edits. Built once and patched, so
 * a remote change never discards a draft or moves focus.
 */
export class TicketDetailView {
  private readonly controller: BacklogController;
  private readonly placeholder: HTMLElement;
  private readonly content: HTMLElement;
  private readonly heading: HTMLHeadingElement;
  private readonly titleInput: HTMLInputElement;
  private readonly titleSlot: HTMLElement;
  private readonly editButton: HTMLButtonElement;
  private readonly archivedNote: HTMLElement;
  private readonly status: Listbox<Status>;
  private readonly priority: Listbox<Priority>;
  private readonly assignee: Listbox<string>;
  private readonly created: HTMLElement;
  private readonly changed: HTMLElement;
  private readonly stale: HTMLElement;
  private readonly staleValues: HTMLDListElement;
  private readonly staleBody: HTMLPreElement;
  private readonly body: HTMLElement;
  private readonly bodyInput: HTMLTextAreaElement;
  private readonly bodySlot: HTMLElement;
  private readonly actions: HTMLElement;
  private readonly saveButton: HTMLButtonElement;
  private readonly cancelButton: HTMLButtonElement;
  private renderedBody: string | null = null;
  private assigneeOptionsKey = "";

  constructor(section: HTMLElement, controller: BacklogController) {
    this.controller = controller;
    this.placeholder = element("p", "ticket-detail-placeholder", "Select a Ticket in the tree to see it here.");
    this.content = element("div", "ticket-detail-content");

    const header = element("div", "ticket-detail-header");
    this.heading = element("h2", "ticket-detail-title");
    this.titleInput = element("input", `${KIT.input} ticket-title-input`);
    this.titleInput.type = "text";
    this.titleInput.required = true;
    // HTML maxLength counts UTF-16 units, while the published limit counts characters.
    this.titleInput.maxLength = MAX_TITLE * 2;
    this.titleInput.setAttribute("aria-label", "Ticket title");
    this.titleInput.addEventListener("input", () => controller.editDraft({ title: this.titleInput.value }));
    this.titleSlot = element("div", "ticket-title-slot");
    this.titleSlot.append(this.heading);
    this.editButton = button("Edit", `${KIT.secondary} ticket-edit`, () => {
      controller.startEdit();
      this.bodyInput.focus();
    });
    header.append(this.titleSlot, this.editButton);
    this.archivedNote = element("p", "ticket-archived-note", "This Ticket is archived.");

    this.status = new Listbox<Status>({
      label: "Status", className: "ticket-status", value: null,
      options: STATUSES.map((value) => ({ value, label: STATUS_LABEL[value] })),
      onChange: (status) => this.change({ status }),
    });
    this.priority = new Listbox<Priority>({
      label: "Priority", className: "ticket-priority", value: null,
      options: PRIORITIES.map((value) => ({ value, label: PRIORITY_LABEL[value] })),
      onChange: (priority) => this.change({ priority }),
    });
    this.assignee = new Listbox<string>({
      label: "Assignee", className: "ticket-assignee", value: UNASSIGNED,
      options: [{ value: UNASSIGNED, label: "Unassigned" }],
      onChange: (value) => this.change({ assignee: value === UNASSIGNED ? null : value }),
    });
    const fields = element("div", "ticket-fields");
    fields.append(field("Status", this.status.element), field("Priority", this.priority.element),
      field("Assignee", this.assignee.element));

    const attributionLines = element("div", "ticket-attribution");
    this.created = element("p", "ticket-created");
    this.changed = element("p", "ticket-changed");
    attributionLines.append(this.created, this.changed);

    this.stale = element("div", `${KIT.notice} ticket-stale`);
    this.stale.setAttribute("role", "status");
    this.staleValues = element("dl", "ticket-stale-values");
    const staleBodyDetails = element("details", "ticket-stale-body-details");
    this.staleBody = element("pre", "ticket-stale-body");
    staleBodyDetails.append(element("summary", "", "Current body"), this.staleBody);
    this.stale.append(
      element("p", "", "This Ticket changed since you started editing. Your draft is kept. Current values:"),
      this.staleValues,
      staleBodyDetails,
      button("Use latest", KIT.secondary, () => void controller.useLatest()),
    );

    this.body = element("div", "ticket-body");
    this.bodyInput = element("textarea", `${KIT.input} ticket-body-input`);
    this.bodyInput.setAttribute("aria-label", "Ticket body (Markdown)");
    this.bodyInput.rows = 14;
    this.bodyInput.addEventListener("input", () => controller.editDraft({ body: this.bodyInput.value }));
    this.bodySlot = element("div", "ticket-body-slot");
    this.bodySlot.append(this.body);

    this.actions = element("div", "ticket-edit-actions");
    this.saveButton = button("Save", KIT.primary, () => void this.save());
    this.cancelButton = button("Cancel", KIT.secondary, () => {
      controller.cancelEdit();
      this.editButton.focus();
    });
    this.actions.append(this.saveButton, this.cancelButton);

    this.content.append(header, this.archivedNote, fields, attributionLines, this.stale, this.bodySlot, this.actions);
    section.append(this.placeholder, this.content);
  }

  render(): void {
    const { detail, busy } = this.controller.state;
    const summary = this.controller.selectedTicket;
    this.placeholder.hidden = detail !== null;
    this.content.hidden = detail === null;
    if (detail === null) return;
    const { ticket, edit } = detail;
    const shown: TicketFields | null = edit?.draft
      ?? (ticket ?? (summary && { ...summary, body: "" })) ?? null;
    const editing = edit !== null;

    this.heading.textContent = ticket?.title ?? summary?.title ?? "";
    this.titleSlot.replaceChildren(editing ? this.titleInput : this.heading);
    if (editing && this.titleInput.value !== edit.draft.title) this.titleInput.value = edit.draft.title;
    this.titleInput.readOnly = busy;
    this.editButton.hidden = editing;
    this.editButton.disabled = ticket === null;
    this.archivedNote.hidden = !(ticket?.archived ?? summary?.archived ?? false);

    this.status.setValue(shown?.status ?? null);
    this.priority.setValue(shown?.priority ?? null);
    const assignee = shown?.assignee ?? null;
    this.setAssigneeOptions(this.controller.assigneeOptions(assignee));
    this.assignee.setValue(assignee ?? UNASSIGNED);
    for (const listbox of [this.status, this.priority, this.assignee]) listbox.disabled = busy || ticket === null;

    this.created.hidden = ticket === null;
    this.changed.hidden = ticket === null;
    if (ticket !== null) {
      attribution(this.created, "Created by", this.controller.labelForIdentity(ticket.creator), ticket.createdAt);
      attribution(this.changed, "Last changed by", this.controller.labelForIdentity(ticket.lastEditor), ticket.lastAt);
    }

    this.renderStale(edit?.stale ?? null);

    this.bodySlot.replaceChildren(editing ? this.bodyInput : this.body);
    if (editing && this.bodyInput.value !== edit.draft.body) this.bodyInput.value = edit.draft.body;
    this.bodyInput.readOnly = busy;
    if (!editing) this.renderBody(ticket);

    this.actions.hidden = !editing;
    this.saveButton.disabled = busy;
    this.cancelButton.disabled = busy;
  }

  dispose(): void {
    this.status.dispose();
    this.priority.dispose();
    this.assignee.dispose();
  }

  private change(changes: Partial<TicketFields>): void {
    this.controller.editDraft(changes);
  }

  private async save(): Promise<void> {
    const saved = await this.controller.saveEdit();
    if (saved && this.editButton.isConnected) this.editButton.focus();
  }

  private setAssigneeOptions(options: readonly AssigneeOption[]): void {
    const key = JSON.stringify(options);
    if (key === this.assigneeOptionsKey) return;
    this.assigneeOptionsKey = key;
    this.assignee.setOptions(options);
  }

  private renderBody(ticket: Ticket | null): void {
    const source = ticket === null ? null : ticket.body;
    if (source === this.renderedBody) return;
    this.renderedBody = source;
    if (source === null) {
      this.body.replaceChildren(element("p", "ticket-body-empty", "Loading…"));
    } else if (source.trim() === "") {
      this.body.replaceChildren(element("p", "ticket-body-empty", "No description yet."));
    } else {
      this.body.innerHTML = renderMarkdown(source);
    }
  }

  private renderStale(current: Ticket | null): void {
    this.stale.hidden = current === null;
    if (current === null) return;
    const rows: [string, string][] = [
      ["Title", current.title],
      ["Status", STATUS_LABEL[current.status]],
      ["Priority", PRIORITY_LABEL[current.priority]],
      ["Assignee", this.controller.labelForAssignee(current.assignee)],
      ["Last changed by", this.controller.labelForIdentity(current.lastEditor)],
    ];
    this.staleValues.replaceChildren(...rows.flatMap(([term, value]) => [element("dt", "", term), element("dd", "", value)]));
    this.staleBody.textContent = current.body;
  }
}
