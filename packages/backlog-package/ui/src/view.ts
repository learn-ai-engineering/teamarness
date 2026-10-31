import type { Epic, Priority, Status, TicketSummary } from "./contract.generated";
import { PRIORITIES, PRIORITY_LABEL, STATUSES, STATUS_LABEL } from "./controller";
import type { BacklogController } from "./controller";
import { Listbox } from "./listbox";

const KIT = {
  primary: "teamarness-control teamarness-control--primary",
  secondary: "teamarness-control teamarness-control--secondary",
  field: "teamarness-field",
  input: "teamarness-input",
  notice: "teamarness-notice",
  noticeDanger: "teamarness-notice teamarness-notice--danger",
} as const;

const AVAILABILITY_LABEL = {
  current: "Up to date",
  catching_up: "Catching up",
  offline: "Offline",
  waiting_for_history_access: "Waiting for history access",
} as const;

/** Titles are at most 256 characters (Backend `MAX_TITLE`). */
const MAX_TITLE = 256;

function button(label: string, className: string, onClick: () => void): HTMLButtonElement {
  const element = document.createElement("button");
  element.type = "button";
  element.className = className;
  element.textContent = label;
  element.addEventListener("click", onClick);
  return element;
}

const ICON_PATHS = {
  plus: "M12 5v14 M5 12h14", x: "M5 5l14 14 M19 5L5 19",
  more: "M5 12h.01 M12 12h.01 M19 12h.01",
  eye: "M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12z M12 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6z",
  eyeOff: "M3 3l18 18 M10.5 5.2A11 11 0 0 1 12 5c6.5 0 10 7 10 7a16 16 0 0 1-3.1 4.1 M6.1 6.1C3.4 8 2 12 2 12s3.5 7 10 7c1.5 0 2.9-.4 4.1-1",
  chevron: "M9 6l6 6-6 6",
} as const;

function icon(name: keyof typeof ICON_PATHS, label: string, onClick: () => void, tooltip = false): HTMLButtonElement {
  const control = button("", "backlog-icon", onClick);
  control.setAttribute("aria-label", label);
  control.title = label;
  control.append(svgIcon(name));
  if (tooltip) {
    control.classList.add("backlog-tooltip-control");
    const hint = document.createElement("span");
    hint.className = "control-tooltip";
    hint.setAttribute("role", "tooltip");
    hint.setAttribute("aria-hidden", "true");
    hint.textContent = label;
    control.append(hint);
  }
  return control;
}

function svgIcon(name: keyof typeof ICON_PATHS): SVGSVGElement {
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("aria-hidden", "true");
  const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
  path.setAttribute("d", ICON_PATHS[name]);
  svg.append(path);
  return svg;
}

function updateIcon(control: HTMLButtonElement, name: keyof typeof ICON_PATHS, label: string): void {
  control.setAttribute("aria-label", label);
  control.title = label;
  control.querySelector("path")?.setAttribute("d", ICON_PATHS[name]);
  const hint = control.querySelector(".control-tooltip");
  if (hint) hint.textContent = label;
}

/** A one-field form built once; the view only patches its disabled state. */
function inlineForm(options: {
  label: string;
  placeholder: string;
  submitLabel: string;
  maxLength: number;
  className: string;
  onSubmit: (value: string) => Promise<boolean>;
  currentDraft?: () => number;
}): { form: HTMLFormElement; input: HTMLInputElement; submit: HTMLButtonElement } {
  const form = document.createElement("form");
  form.className = `${options.className} ${KIT.field}`;
  const input = document.createElement("input");
  input.className = KIT.input;
  input.type = "text";
  input.required = true;
  // HTML maxLength counts UTF-16 units, while the published limit counts characters.
  input.maxLength = options.maxLength * 2;
  let inputRevision = 0;
  input.addEventListener("input", () => { inputRevision++; input.setCustomValidity(""); });
  input.placeholder = options.placeholder;
  input.setAttribute("aria-label", options.label);
  const submit = document.createElement("button");
  submit.type = "submit";
  submit.className = KIT.primary;
  submit.textContent = options.submitLabel;
  form.append(input, submit);
  form.addEventListener("submit", (event) => {
    event.preventDefault();
    const value = input.value.trim();
    if (!value) return;
    if ([...value].length > options.maxLength) {
      input.setCustomValidity(`${options.label} must be at most ${options.maxLength} characters.`);
      input.reportValidity();
      return;
    }
    const revision = inputRevision;
    const draft = options.currentDraft?.();
    void options.onSubmit(value).then((ok) => {
      if (ok && revision === inputRevision && draft === options.currentDraft?.()) input.value = "";
    });
  });
  return { form, input, submit };
}

type FilterValue<V extends string> = V | "all";

/**
 * Builds the Backlog DOM once and updates regions in place. The create forms and filter
 * dropdowns persist across renders so a remote change never discards what a Member is doing.
 */
export class BacklogView {
  private readonly page: HTMLElement;
  private readonly status: HTMLElement;
  private readonly warnings: HTMLElement;
  private readonly error: HTMLElement;
  private readonly epicList: HTMLUListElement;
  private readonly epicForm: ReturnType<typeof inlineForm>;
  private readonly ticketForm: ReturnType<typeof inlineForm>;
  private readonly archivedToggle: HTMLButtonElement;
  private readonly priorityFilter: Listbox<FilterValue<Priority>>;
  private readonly statusFilter: Listbox<FilterValue<Status>>;
  private readonly clearPriority: HTMLButtonElement;
  private readonly clearStatus: HTMLButtonElement;
  private readonly detailTitle: HTMLElement;
  private readonly detailPlaceholder: HTMLElement;
  private readonly controller: BacklogController;
  /** `"epic"` while the New Epic form shows, an Epic ID while its Add Ticket form shows. */
  private createTarget: string | null = null;
  private createGeneration = 0;
  private activeMenu: HTMLElement | null = null;
  private activeMenuButton: HTMLButtonElement | null = null;
  private menuListbox: Listbox<string> | null = null;
  private renameForm: ReturnType<typeof inlineForm> | null = null;
  private renameTarget: string | null = null;
  private readonly onOutsideClick = (event: Event): void => {
    if (this.activeMenu && !event.composedPath().includes(this.activeMenu)
      && !event.composedPath().includes(this.activeMenuButton as EventTarget)) this.dismissMenu();
  };
  private readonly onEscape = (event: Event): void => {
    if ((event as KeyboardEvent).key === "Escape" && this.activeMenu) {
      const trigger = this.activeMenuButton;
      this.dismissMenu();
      trigger?.focus();
      (event as KeyboardEvent).preventDefault();
    }
  };

  constructor(root: ShadowRoot | HTMLElement, controller: BacklogController, title: string) {
    this.controller = controller;
    this.page = document.createElement("div");
    this.page.className = "teamarness-page backlog-page";

    const header = document.createElement("header");
    header.className = "teamarness-page-header";
    const heading = document.createElement("h1");
    heading.className = "teamarness-page-title";
    heading.textContent = title;
    this.status = document.createElement("p");
    this.status.className = "availability";
    this.status.setAttribute("role", "status");
    header.append(heading, this.status);

    this.warnings = document.createElement("div");
    this.warnings.className = "backlog-warnings";
    this.error = document.createElement("p");
    this.error.className = KIT.noticeDanger;
    this.error.setAttribute("role", "alert");
    this.error.hidden = true;

    const shell = document.createElement("div");
    shell.className = "backlog-shell";

    const rail = document.createElement("aside");
    rail.className = "epic-rail";
    rail.setAttribute("aria-label", "Epics and Tickets");
    const railHeader = document.createElement("div");
    railHeader.className = "epic-rail-header";
    const railHeading = document.createElement("h2");
    railHeading.textContent = "Epics";
    this.archivedToggle = icon("eyeOff", "Show archived", () => controller.toggleArchived(), true);
    this.archivedToggle.classList.add("archived-toggle");
    this.archivedToggle.setAttribute("aria-pressed", "false");
    const addEpic = icon("plus", "New Epic", () => {
      this.createGeneration++;
      this.createTarget = "epic";
      this.render();
      this.epicForm.input.focus();
    });
    addEpic.classList.add("epic-add");
    railHeader.append(railHeading, this.archivedToggle, addEpic);

    // --- filters (built once) ---
    const filters = document.createElement("div");
    filters.className = "backlog-filters";
    this.priorityFilter = new Listbox<FilterValue<Priority>>({
      label: "Priority filter",
      className: "priority-filter",
      value: "all",
      options: [{ value: "all", label: "All priorities" }, ...PRIORITIES.map((value) => ({ value, label: PRIORITY_LABEL[value] }))],
      onChange: (value) => controller.setPriorityFilter(value === "all" ? null : value),
    });
    this.statusFilter = new Listbox<FilterValue<Status>>({
      label: "Status filter",
      className: "status-filter",
      value: "all",
      options: [{ value: "all", label: "All statuses" }, ...STATUSES.map((value) => ({ value, label: STATUS_LABEL[value] }))],
      onChange: (value) => controller.setStatusFilter(value === "all" ? null : value),
    });
    this.clearPriority = icon("x", "Clear priority filter", () => {
      controller.setPriorityFilter(null);
      this.priorityFilter.button.focus();
    });
    this.clearPriority.classList.add("filter-clear");
    this.clearStatus = icon("x", "Clear status filter", () => {
      controller.setStatusFilter(null);
      this.statusFilter.button.focus();
    });
    this.clearStatus.classList.add("filter-clear");
    const priorityGroup = document.createElement("div");
    priorityGroup.className = "backlog-filter";
    priorityGroup.append(this.priorityFilter.element, this.clearPriority);
    const statusGroup = document.createElement("div");
    statusGroup.className = "backlog-filter";
    statusGroup.append(this.statusFilter.element, this.clearStatus);
    filters.append(priorityGroup, statusGroup);

    this.epicList = document.createElement("ul");
    this.epicList.className = "epic-list";
    this.epicForm = inlineForm({
      label: "New Epic title", placeholder: "New Epic", submitLabel: "Add",
      maxLength: MAX_TITLE, className: "epic-create", currentDraft: () => this.createGeneration,
      onSubmit: async (value) => {
        const generation = this.createGeneration;
        const ok = await controller.createEpic(value);
        if (ok && generation === this.createGeneration && this.createTarget === "epic") {
          this.createTarget = null;
          this.render();
        }
        return ok;
      },
    });
    this.ticketForm = inlineForm({
      label: "New Ticket title", placeholder: "New Ticket title", submitLabel: "Create",
      maxLength: MAX_TITLE, className: "ticket-create", currentDraft: () => this.createGeneration,
      onSubmit: async (value) => {
        const target = this.createTarget;
        const generation = this.createGeneration;
        if (target === null || target === "epic") return false;
        const ok = await controller.createTicket(target, value);
        if (ok && generation === this.createGeneration && this.createTarget === target) {
          this.createTarget = null;
          this.render();
        }
        return ok;
      },
    });
    this.epicForm.input.addEventListener("input", () => { this.createGeneration++; });
    this.ticketForm.input.addEventListener("input", () => { this.createGeneration++; });
    const cancelCreate = () => { this.createGeneration++; this.createTarget = null; this.render(); };
    this.epicForm.form.append(icon("x", "Cancel", cancelCreate));
    this.ticketForm.form.append(icon("x", "Cancel", cancelCreate));
    rail.append(railHeader, filters, this.epicList);

    // --- detail pane (Ticket detail arrives in a later slice) ---
    const detail = document.createElement("section");
    detail.className = "ticket-detail";
    detail.setAttribute("aria-label", "Ticket");
    this.detailTitle = document.createElement("h2");
    this.detailTitle.className = "ticket-detail-title";
    this.detailPlaceholder = document.createElement("p");
    this.detailPlaceholder.className = "ticket-detail-placeholder";
    detail.append(this.detailTitle, this.detailPlaceholder);

    shell.append(rail, detail);
    this.page.append(header, this.warnings, this.error, shell);
    root.addEventListener("click", this.onOutsideClick);
    root.addEventListener("keydown", this.onEscape);
    document.addEventListener("click", this.onOutsideClick);
    root.append(this.page);
  }

  render(): void {
    const { snapshot, busy, error, showArchived, filters, loaded } = this.controller.state;
    this.status.textContent = AVAILABILITY_LABEL[snapshot.availability];
    updateIcon(this.archivedToggle, showArchived ? "eye" : "eyeOff", showArchived ? "Hide archived" : "Show archived");
    this.archivedToggle.setAttribute("aria-pressed", String(showArchived));
    this.warnings.replaceChildren(...snapshot.warnings.map((text) => {
      const notice = document.createElement("p");
      notice.className = KIT.notice;
      notice.textContent = text;
      return notice;
    }));
    this.error.hidden = error === null;
    this.error.textContent = error ?? "";
    this.priorityFilter.setValue(filters.priority ?? "all");
    this.statusFilter.setValue(filters.status ?? "all");
    this.clearPriority.hidden = filters.priority === null;
    this.clearStatus.hidden = filters.status === null;
    this.epicForm.submit.disabled = busy;
    if (this.createTarget !== null && this.createTarget !== "epic"
      && !this.controller.visibleEpics.some((epic) => epic.epicId === this.createTarget && !epic.archived)) {
      this.createGeneration++;
      this.createTarget = null;
      this.ticketForm.input.value = "";
    }
    this.ticketForm.submit.disabled = busy;
    this.renderTree(loaded);
    this.renderDetail();
  }

  dispose(): void {
    this.dismissMenu();
    this.priorityFilter.dispose();
    this.statusFilter.dispose();
    this.page.getRootNode().removeEventListener("click", this.onOutsideClick);
    this.page.getRootNode().removeEventListener("keydown", this.onEscape);
    document.removeEventListener("click", this.onOutsideClick);
    this.page.remove();
  }

  private renderDetail(): void {
    const ticket = this.controller.selectedTicket;
    this.detailTitle.textContent = ticket?.title ?? "Choose a Ticket";
    this.detailPlaceholder.textContent = ticket === null
      ? "Select a Ticket in the tree to see it here."
      : `${PRIORITY_LABEL[ticket.priority]} priority · ${STATUS_LABEL[ticket.status]}`;
  }

  private dismissMenu(): void {
    this.menuListbox?.close(false);
    this.menuListbox = null;
    this.activeMenu?.remove();
    this.activeMenuButton?.setAttribute("aria-expanded", "false");
    this.activeMenu = null;
    this.activeMenuButton = null;
  }

  private menu(row: HTMLElement, trigger: HTMLButtonElement, actions: HTMLElement, listbox: Listbox<string> | null): void {
    trigger.setAttribute("aria-expanded", "false");
    trigger.addEventListener("click", () => {
      if (this.activeMenuButton === trigger) { this.dismissMenu(); return; }
      this.dismissMenu();
      row.append(actions);
      this.activeMenu = actions;
      this.activeMenuButton = trigger;
      this.menuListbox = listbox;
      trigger.setAttribute("aria-expanded", "true");
      (actions.querySelector("button") as HTMLButtonElement | null)?.focus();
    });
  }

  private startRename(epic: Epic): void {
    this.dismissMenu();
    this.renameTarget = epic.epicId;
    const rename = inlineForm({
      label: "Epic title", placeholder: "Epic title", submitLabel: "Save", maxLength: MAX_TITLE,
      className: "epic-rename",
      onSubmit: async (value) => {
        const ok = await this.controller.renameEpic(epic.epicId, value);
        if (ok) { this.renameTarget = null; this.renameForm = null; }
        this.render();
        return ok;
      },
    });
    rename.input.value = epic.title;
    rename.form.append(icon("x", "Cancel", () => {
      this.renameTarget = null;
      this.renameForm = null;
      this.render();
    }));
    this.renameForm = rename;
    this.render();
    rename.input.focus();
  }

  private renderTree(loaded: boolean): void {
    const root = this.epicList.getRootNode() as ShadowRoot | Document;
    const focused = root.activeElement;
    const inTree = focused !== null && this.epicList.contains(focused);
    const focusKey = inTree ? (focused as HTMLElement).dataset.treeFocus
      ?? this.activeMenuButton?.dataset.treeFocus : undefined;
    const selection = inTree && focused instanceof HTMLInputElement
      ? { start: focused.selectionStart, end: focused.selectionEnd } : null;
    this.dismissMenu();
    const epics = this.controller.visibleEpics;
    const rows = epics.map((epic) => this.epicRow(epic));
    this.epicList.replaceChildren(...rows);
    if (epics.length === 0 && this.createTarget !== "epic") {
      const empty = document.createElement("li");
      empty.className = "backlog-empty";
      empty.textContent = loaded ? "No Epics yet." : "Loading…";
      this.epicList.append(empty);
    }
    if (this.createTarget === "epic") {
      const create = document.createElement("li");
      create.append(this.epicForm.form);
      this.epicList.prepend(create);
    }
    if (inTree) {
      const next = focused instanceof HTMLElement && this.epicList.contains(focused) ? focused
        : [...this.epicList.querySelectorAll<HTMLElement>("[data-tree-focus]")]
          .find((control) => control.dataset.treeFocus === focusKey);
      next?.focus();
      if (next === focused && selection && focused instanceof HTMLInputElement
        && selection.start !== null && selection.end !== null) {
        focused.setSelectionRange(selection.start, selection.end);
      }
    }
  }

  private epicRow(epic: Epic): HTMLLIElement {
    const { busy, expandedEpicIds, filters } = this.controller.state;
    const item = document.createElement("li");
    item.className = "epic-group";
    const row = document.createElement("div");
    row.className = "tree-row";
    if (epic.archived) row.classList.add("is-archived");
    const expanded = expandedEpicIds.has(epic.epicId);
    const toggle = button("", "epic-button", () => this.controller.toggleEpic(epic.epicId));
    toggle.append(svgIcon("chevron"));
    const label = document.createElement("span");
    label.className = "epic-title";
    label.textContent = epic.title + (epic.archived ? " (archived)" : "");
    toggle.append(label);
    toggle.setAttribute("aria-expanded", String(expanded));
    toggle.dataset.treeFocus = `epic:toggle:${epic.epicId}`;
    const trigger = icon("more", `Actions for ${epic.title}`, () => {});
    trigger.dataset.treeFocus = `epic:actions:${epic.epicId}`;
    const actions = document.createElement("div");
    actions.className = "epic-actions action-menu";
    actions.setAttribute("role", "group");
    actions.setAttribute("aria-label", `Actions for ${epic.title}`);
    actions.append(
      button("Rename", KIT.secondary, () => this.startRename(epic)),
      button(epic.archived ? "Restore" : "Archive", KIT.secondary, () => {
        this.dismissMenu();
        void (epic.archived ? this.controller.restoreEpic(epic.epicId) : this.controller.archiveEpic(epic.epicId));
      }),
    );
    for (const control of actions.querySelectorAll("button")) control.disabled = busy;
    this.menu(item, trigger, actions, null);
    row.append(toggle);
    if (!epic.archived) {
      const add = icon("plus", `Add Ticket to ${epic.title}`, () => {
        this.controller.expandEpic(epic.epicId);
        this.createGeneration++;
        this.createTarget = epic.epicId;
        this.render();
        this.ticketForm.input.focus();
      });
      add.dataset.treeFocus = `epic:add:${epic.epicId}`;
      row.append(add);
    }
    row.append(trigger);
    item.append(row);
    if (this.renameTarget === epic.epicId && this.renameForm) item.append(this.renameForm.form);
    if (!expanded) return item;
    const list = document.createElement("ul");
    list.className = "ticket-list";
    list.setAttribute("aria-label", `Tickets in ${epic.title}`);
    const tickets = this.controller.visibleTickets(epic.epicId);
    list.append(...tickets.map((ticket) => this.ticketRow(ticket)));
    if (tickets.length === 0) {
      const empty = document.createElement("li");
      empty.className = "backlog-empty";
      empty.textContent = filters.priority !== null || filters.status !== null ? "No matching Tickets." : "No Tickets yet.";
      list.append(empty);
    }
    if (this.createTarget === epic.epicId) {
      const create = document.createElement("li");
      create.append(this.ticketForm.form);
      list.append(create);
    }
    item.append(list);
    return item;
  }

  private ticketRow(ticket: TicketSummary): HTMLLIElement {
    const { busy, selectedTicketId } = this.controller.state;
    const row = document.createElement("li");
    row.className = "ticket-row";
    row.dataset.ticketId = ticket.ticketId;
    if (ticket.archived) row.classList.add("is-archived");
    const select = button("", "ticket-title", () => this.controller.selectTicket(ticket.ticketId));
    const label = document.createElement("span");
    label.className = "ticket-title-text";
    label.textContent = ticket.title + (ticket.archived ? " (archived)" : "");
    const meta = document.createElement("span");
    meta.className = `ticket-meta priority-${ticket.priority}`;
    meta.textContent = `${PRIORITY_LABEL[ticket.priority]} · ${STATUS_LABEL[ticket.status]}`;
    select.append(label, meta);
    select.dataset.treeFocus = `ticket:select:${ticket.ticketId}`;
    if (selectedTicketId === ticket.ticketId) select.setAttribute("aria-current", "true");
    const trigger = icon("more", `Actions for ${ticket.title}`, () => {});
    trigger.dataset.treeFocus = `ticket:actions:${ticket.ticketId}`;
    const actions = document.createElement("div");
    actions.className = "ticket-actions action-menu";
    actions.setAttribute("role", "group");
    actions.setAttribute("aria-label", `Actions for ${ticket.title}`);
    let move: Listbox<string> | null = null;
    const destinations = this.controller.moveDestinations(ticket);
    if (destinations.length > 0) {
      move = new Listbox<string>({
        label: `Move ${ticket.title} to`,
        className: "ticket-move",
        placeholder: "Move to…",
        value: null,
        options: destinations.map((epic) => ({ value: epic.epicId, label: epic.title })),
        onChange: (epicId) => {
          this.dismissMenu();
          if (trigger.isConnected) trigger.focus();
          void this.controller.moveTicket(ticket.ticketId, epicId);
        },
      });
      move.disabled = busy;
      actions.append(move.element);
    }
    const archive = button(ticket.archived ? "Restore" : "Archive", KIT.secondary, () => {
      this.dismissMenu();
      void (ticket.archived ? this.controller.restoreTicket(ticket.ticketId) : this.controller.archiveTicket(ticket.ticketId));
    });
    archive.disabled = busy;
    actions.append(archive);
    this.menu(row, trigger, actions, move);
    row.append(select, trigger);
    return row;
  }
}
