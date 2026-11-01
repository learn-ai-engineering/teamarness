/**
 * A select-only combobox (WAI-ARIA Authoring Practices): a button that opens a popup listbox.
 *
 * DOM focus stays on the button; the active option is announced through
 * `aria-activedescendant`. The popup closes on Escape, on a choice or on a click outside it, and
 * focus is returned to the button. The Backlog UI never uses a native `<select>`.
 */

export type ListboxOption<V extends string> = Readonly<{ value: V; label: string }>;

export type ListboxOptions<V extends string> = Readonly<{
  /** The accessible name of the button, such as "Priority filter". */
  label: string;
  options: readonly ListboxOption<V>[];
  /** The chosen value, or `null` to show `placeholder`. */
  value: V | null;
  /** Shown on the button while no value is chosen. */
  placeholder?: string;
  /** Called when a Member chooses an option, including the one already chosen. */
  onChange: (value: V) => void;
  /** Extra classes for the wrapper element. */
  className?: string;
}>;

const TYPEAHEAD_RESET_MS = 500;
let nextId = 0;

const CHEVRON = "M6 9l6 6 6-6";

export class Listbox<V extends string> {
  /** The wrapper holding the button and, while open, the popup. */
  readonly element: HTMLElement;
  readonly button: HTMLButtonElement;
  private readonly popup: HTMLUListElement;
  private readonly valueLabel: HTMLSpanElement;
  private readonly id = `backlog-listbox-${++nextId}`;
  private readonly onChange: (value: V) => void;
  private readonly placeholder: string;
  private options: readonly ListboxOption<V>[];
  private chosen: V | null;
  private active = -1;
  private typed = "";
  private typedAt = 0;
  private listening: { root: Node; document: Document } | null = null;

  private readonly onOutsideClick = (event: Event): void => {
    if (event.composedPath().includes(this.element)) return;
    // Focus comes back to the button unless the click moved it to another control.
    const root = this.element.getRootNode() as Document | ShadowRoot;
    const active = root.activeElement;
    this.close(active === null || active === this.button || active === document.body);
  };

  constructor(options: ListboxOptions<V>) {
    this.options = options.options;
    this.chosen = options.value;
    this.onChange = options.onChange;
    this.placeholder = options.placeholder ?? "";

    this.element = document.createElement("div");
    this.element.className = `backlog-listbox${options.className ? ` ${options.className}` : ""}`;

    this.button = document.createElement("button");
    this.button.type = "button";
    this.button.className = "teamarness-input backlog-listbox-button";
    this.button.id = `${this.id}-button`;
    this.button.setAttribute("role", "combobox");
    this.button.setAttribute("aria-haspopup", "listbox");
    this.button.setAttribute("aria-expanded", "false");
    this.button.setAttribute("aria-controls", `${this.id}-listbox`);
    this.button.setAttribute("aria-label", options.label);
    this.valueLabel = document.createElement("span");
    this.valueLabel.className = "backlog-listbox-value";
    const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    svg.setAttribute("viewBox", "0 0 24 24");
    svg.setAttribute("aria-hidden", "true");
    svg.classList.add("backlog-listbox-chevron");
    const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
    path.setAttribute("d", CHEVRON);
    svg.append(path);
    this.button.append(this.valueLabel, svg);
    this.button.addEventListener("click", () => {
      if (this.isOpen) this.close(true);
      else this.open(this.selectedIndex());
    });
    this.button.addEventListener("keydown", (event) => this.onKeyDown(event));

    this.popup = document.createElement("ul");
    this.popup.className = "backlog-listbox-popup";
    this.popup.id = `${this.id}-listbox`;
    this.popup.setAttribute("role", "listbox");
    this.popup.setAttribute("aria-label", options.label);
    this.popup.tabIndex = -1;
    this.popup.hidden = true;
    // A press on an option must not move focus off the button.
    this.popup.addEventListener("mousedown", (event) => event.preventDefault());

    this.element.append(this.button, this.popup);
    this.renderOptions();
    this.renderButton();
  }

  get value(): V | null {
    return this.chosen;
  }

  get isOpen(): boolean {
    return !this.popup.hidden;
  }

  set disabled(disabled: boolean) {
    this.button.disabled = disabled;
    if (disabled) this.close(false);
  }

  /** Shows `value` as chosen without calling `onChange`. */
  setValue(value: V | null): void {
    if (value === this.chosen) return;
    this.chosen = value;
    this.renderButton();
    this.renderOptions();
  }

  /** Replaces the options; an unknown chosen value shows the placeholder. */
  setOptions(options: readonly ListboxOption<V>[]): void {
    this.options = options;
    if (this.active >= options.length) this.active = options.length - 1;
    this.renderOptions();
    this.renderButton();
  }

  open(activeIndex: number): void {
    if (this.button.disabled || this.options.length === 0) return;
    this.popup.hidden = false;
    this.button.setAttribute("aria-expanded", "true");
    this.setActive(Math.max(0, Math.min(this.options.length - 1, activeIndex)));
    if (this.listening === null) {
      const root = this.element.getRootNode();
      this.listening = { root, document };
      root.addEventListener("click", this.onOutsideClick);
      if (root !== document) document.addEventListener("click", this.onOutsideClick);
    }
  }

  /** Closes the popup without choosing; `returnFocus` moves focus back to the button. */
  close(returnFocus: boolean): void {
    if (this.listening !== null) {
      this.listening.root.removeEventListener("click", this.onOutsideClick);
      this.listening.document.removeEventListener("click", this.onOutsideClick);
      this.listening = null;
    }
    if (!this.isOpen) return;
    this.popup.hidden = true;
    this.button.setAttribute("aria-expanded", "false");
    this.button.removeAttribute("aria-activedescendant");
    if (returnFocus) this.button.focus();
  }

  dispose(): void {
    this.close(false);
    this.element.remove();
  }

  private selectedIndex(): number {
    const index = this.options.findIndex((option) => option.value === this.chosen);
    return index < 0 ? 0 : index;
  }

  private choose(index: number): void {
    const option = this.options[index];
    this.close(true);
    if (option === undefined) return;
    this.chosen = option.value;
    this.renderButton();
    this.renderOptions();
    this.onChange(option.value);
  }

  private setActive(index: number): void {
    this.active = index;
    const items = this.popup.children;
    for (let position = 0; position < items.length; position += 1) {
      items.item(position)?.classList.toggle("is-active", position === index);
    }
    const item = items[index] as HTMLElement | undefined;
    if (item) {
      this.button.setAttribute("aria-activedescendant", item.id);
      item.scrollIntoView?.({ block: "nearest" });
    }
  }

  private onKeyDown(event: KeyboardEvent): void {
    const last = this.options.length - 1;
    const handled = (): void => { event.preventDefault(); event.stopPropagation(); };
    if (!this.isOpen) {
      switch (event.key) {
        case "ArrowDown": case "ArrowUp": case "Enter": case " ":
          handled(); this.open(this.selectedIndex()); return;
        case "Home": handled(); this.open(0); return;
        case "End": handled(); this.open(last); return;
      }
      if (this.isTypeahead(event)) { handled(); this.open(this.selectedIndex()); this.typeahead(event.key); }
      return;
    }
    switch (event.key) {
      case "ArrowDown":
        handled();
        if (event.altKey) return;
        this.setActive(Math.min(last, this.active + 1)); return;
      case "ArrowUp":
        handled();
        if (event.altKey) { this.choose(this.active); return; }
        this.setActive(Math.max(0, this.active - 1)); return;
      case "Home": handled(); this.setActive(0); return;
      case "End": handled(); this.setActive(last); return;
      case "PageDown": handled(); this.setActive(Math.min(last, this.active + 10)); return;
      case "PageUp": handled(); this.setActive(Math.max(0, this.active - 10)); return;
      case "Enter": case " ":
        // A space while typing a name continues the typeahead.
        if (event.key === " " && this.typed !== "" && Date.now() - this.typedAt < TYPEAHEAD_RESET_MS) {
          handled(); this.typeahead(" "); return;
        }
        handled(); this.choose(this.active); return;
      case "Escape": handled(); this.close(true); return;
      case "Tab": this.choose(this.active); return;
    }
    if (this.isTypeahead(event)) { handled(); this.typeahead(event.key); }
  }

  private isTypeahead(event: KeyboardEvent): boolean {
    return event.key.length === 1 && event.key !== " " && !event.ctrlKey && !event.metaKey && !event.altKey;
  }

  /** Moves to the next option whose label starts with what was typed recently. */
  private typeahead(key: string): void {
    const now = Date.now();
    const fresh = now - this.typedAt > TYPEAHEAD_RESET_MS;
    this.typed = (fresh ? "" : this.typed) + key.toLowerCase();
    this.typedAt = now;
    const labels = this.options.map((option) => option.label.toLowerCase());
    // Repeating one letter cycles through the options starting with it.
    const repeated = [...this.typed].every((char) => char === this.typed[0]);
    const search = repeated ? this.typed.slice(0, 1) : this.typed;
    const start = repeated || this.typed.length === 1 ? this.active + 1 : this.active;
    for (let step = 0; step < labels.length; step += 1) {
      const index = (Math.max(0, start) + step) % labels.length;
      if (labels[index]?.startsWith(search)) { this.setActive(index); return; }
    }
  }

  private renderButton(): void {
    const option = this.options.find((candidate) => candidate.value === this.chosen);
    this.valueLabel.textContent = option?.label ?? this.placeholder;
    this.valueLabel.classList.toggle("is-placeholder", option === undefined);
  }

  private renderOptions(): void {
    this.popup.replaceChildren(...this.options.map((option, index) => {
      const item = document.createElement("li");
      item.id = `${this.id}-option-${index}`;
      item.className = "backlog-listbox-option";
      item.setAttribute("role", "option");
      item.setAttribute("aria-selected", String(option.value === this.chosen));
      item.dataset.value = option.value;
      item.textContent = option.label;
      if (index === this.active && this.isOpen) item.classList.add("is-active");
      item.addEventListener("click", () => this.choose(index));
      item.addEventListener("mousemove", () => { if (this.active !== index) this.setActive(index); });
      return item;
    }));
  }
}
