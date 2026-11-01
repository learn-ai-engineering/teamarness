// @vitest-environment happy-dom

import { afterEach, describe, expect, it, vi } from "vitest";
import { Listbox } from "./listbox";

type Fruit = "apple" | "apricot" | "banana" | "blueberry" | "cherry";

const FRUIT: { value: Fruit; label: string }[] = [
  { value: "apple", label: "Apple" },
  { value: "apricot", label: "Apricot" },
  { value: "banana", label: "Banana" },
  { value: "blueberry", label: "Blueberry" },
  { value: "cherry", label: "Cherry" },
];

let cleanup: (() => void)[] = [];

afterEach(() => {
  cleanup.forEach((step) => step());
  cleanup = [];
  vi.useRealTimers();
});

/** A listbox mounted in a shadow root, as a Package UI mounts it. */
function setup(value: Fruit | null = "banana") {
  const host = document.createElement("div");
  document.body.append(host);
  const root = host.attachShadow({ mode: "open" });
  const outside = document.createElement("button");
  outside.textContent = "Elsewhere";
  root.append(outside);
  const onChange = vi.fn<(value: Fruit) => void>();
  const listbox = new Listbox<Fruit>({ label: "Fruit", options: FRUIT, value, placeholder: "Pick one", onChange });
  root.append(listbox.element);
  cleanup.push(() => { listbox.dispose(); host.remove(); });
  const popup = root.querySelector('[role="listbox"]') as HTMLElement;
  const options = () => [...root.querySelectorAll<HTMLElement>('[role="option"]')];
  const press = (key: string, init: KeyboardEventInit = {}) => {
    listbox.button.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, composed: true, cancelable: true, ...init }));
  };
  const activeLabel = () => {
    const id = listbox.button.getAttribute("aria-activedescendant");
    return id === null ? null : root.getElementById(id)?.textContent ?? null;
  };
  return { root, listbox, popup, options, press, activeLabel, onChange, outside };
}

describe("Listbox (select-only combobox)", () => {
  it("carries the combobox and listbox ARIA attributes", () => {
    const { listbox, popup, options } = setup();
    const control = listbox.button;
    expect(control.tagName).toBe("BUTTON");
    expect(control.getAttribute("role")).toBe("combobox");
    expect(control.getAttribute("aria-haspopup")).toBe("listbox");
    expect(control.getAttribute("aria-expanded")).toBe("false");
    expect(control.getAttribute("aria-label")).toBe("Fruit");
    expect(control.getAttribute("aria-controls")).toBe(popup.id);
    expect(control.querySelector("svg")).not.toBeNull();
    expect(control.textContent).toBe("Banana");
    expect(popup.hidden).toBe(true);
    expect(options().map((option) => option.getAttribute("aria-selected")))
      .toEqual(["false", "false", "true", "false", "false"]);
    expect(new Set(options().map((option) => option.id)).size).toBe(5);
    expect(listbox.element.querySelector("select")).toBeNull();
  });

  it("shows the placeholder when nothing is chosen", () => {
    const { listbox } = setup(null);
    expect(listbox.button.textContent).toBe("Pick one");
    expect(listbox.button.querySelector(".is-placeholder")).not.toBeNull();
  });

  it("opens on click with the chosen option active and chooses an option on click", () => {
    const { listbox, popup, options, activeLabel, onChange } = setup();
    listbox.button.click();
    expect(popup.hidden).toBe(false);
    expect(listbox.button.getAttribute("aria-expanded")).toBe("true");
    expect(activeLabel()).toBe("Banana");
    options()[4]!.click();
    expect(onChange).toHaveBeenCalledWith("cherry");
    expect(popup.hidden).toBe(true);
    expect(listbox.value).toBe("cherry");
    expect(listbox.button.textContent).toBe("Cherry");
    expect(options()[4]!.getAttribute("aria-selected")).toBe("true");
  });

  it("moves with Up, Down, Home and End and chooses with Enter", () => {
    const { listbox, press, activeLabel, onChange, popup } = setup();
    listbox.button.focus();
    press("ArrowDown");
    expect(popup.hidden).toBe(false);
    expect(activeLabel()).toBe("Banana");
    press("ArrowDown");
    expect(activeLabel()).toBe("Blueberry");
    press("ArrowUp");
    press("ArrowUp");
    expect(activeLabel()).toBe("Apricot");
    press("Home");
    expect(activeLabel()).toBe("Apple");
    press("ArrowUp");
    expect(activeLabel()).toBe("Apple");
    press("End");
    expect(activeLabel()).toBe("Cherry");
    press("ArrowDown");
    expect(activeLabel()).toBe("Cherry");
    press("Enter");
    expect(onChange).toHaveBeenLastCalledWith("cherry");
    expect(popup.hidden).toBe(true);
    expect(listbox.button.getAttribute("aria-activedescendant")).toBeNull();
  });

  it("opens with Home or End on the first or last option and chooses with Space", () => {
    const { press, activeLabel, onChange } = setup();
    press("End");
    expect(activeLabel()).toBe("Cherry");
    press("Escape");
    press("Home");
    expect(activeLabel()).toBe("Apple");
    press(" ");
    expect(onChange).toHaveBeenLastCalledWith("apple");
  });

  it("closes on Escape without choosing and keeps focus on the button", () => {
    const { root, listbox, press, onChange, popup } = setup();
    listbox.button.focus();
    press("Enter");
    press("ArrowDown");
    press("Escape");
    expect(popup.hidden).toBe(true);
    expect(onChange).not.toHaveBeenCalled();
    expect(listbox.value).toBe("banana");
    expect(root.activeElement).toBe(listbox.button);
  });

  it("does not let Escape on an open popup reach enclosing handlers", () => {
    const { root, press } = setup();
    const outer = vi.fn();
    root.addEventListener("keydown", outer);
    press("Enter");
    press("Escape");
    expect(outer).not.toHaveBeenCalled();
    press("Escape");
    expect(outer).toHaveBeenCalledTimes(1);
  });

  it("finds options by typeahead, cycling on a repeated letter", () => {
    vi.useFakeTimers();
    const { press, activeLabel, onChange } = setup();
    press("c");
    expect(activeLabel()).toBe("Cherry");
    vi.advanceTimersByTime(1000);
    press("a");
    expect(activeLabel()).toBe("Apple");
    press("a");
    expect(activeLabel()).toBe("Apricot");
    vi.advanceTimersByTime(1000);
    press("b");
    press("l");
    expect(activeLabel()).toBe("Blueberry");
    press("Enter");
    expect(onChange).toHaveBeenLastCalledWith("blueberry");
  });

  it("closes on an outside click and returns focus to the button", () => {
    const { root, listbox, popup, onChange } = setup();
    listbox.button.focus();
    listbox.button.click();
    expect(popup.hidden).toBe(false);
    document.body.click();
    expect(popup.hidden).toBe(true);
    expect(listbox.button.getAttribute("aria-expanded")).toBe("false");
    expect(root.activeElement).toBe(listbox.button);
    expect(onChange).not.toHaveBeenCalled();
  });

  it("closes on a click elsewhere in the same shadow root", () => {
    const { listbox, popup, outside } = setup();
    listbox.button.click();
    outside.click();
    expect(popup.hidden).toBe(true);
  });

  it("returns focus to the button after a choice", () => {
    const { root, listbox, options } = setup();
    listbox.button.click();
    options()[0]!.click();
    expect(root.activeElement).toBe(listbox.button);
  });

  it("updates without calling onChange and stays closed while disabled", () => {
    const { listbox, popup, onChange } = setup();
    listbox.setValue("apple");
    expect(listbox.button.textContent).toBe("Apple");
    listbox.disabled = true;
    listbox.button.click();
    expect(popup.hidden).toBe(true);
    expect(onChange).not.toHaveBeenCalled();
  });
});
