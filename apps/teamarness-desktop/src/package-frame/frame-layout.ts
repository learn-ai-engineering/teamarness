import { MAX_FRAME_EXTENT_PX } from "./protocol";

/** The part of the frame the Person can see, in the frame's CSS pixels. */
export type VisibleRegion = Readonly<{ top: number; height: number }>;

/** Settings-view layout inside a Package frame: content sizing and modal placement. */
export type SettingsLayout = Readonly<{
  setVisibleRegion(region: VisibleRegion): void;
  stop(): void;
}>;

export type SettingsLayoutInput = Readonly<{
  document: Document;
  host: HTMLElement;
  root: ShadowRoot;
  reportHeight(height: number): void;
}>;

/** Space kept between a placed modal dialog and the edge of the visible region. */
const DIALOG_GUTTER_PX = 16;

/**
 * Lets a settings view's document shrink to its content, so the Shell can size the frame to it.
 * Without this the frame document fills the frame, and a frame sized to its document could
 * grow but never shrink.
 */
const CONTENT_SIZED_CSS = "html, body { height: auto; overflow: hidden; } #teamarness-package-host { min-height: 0; display: flow-root; }";

/**
 * Starts settings-view layout. The frame reports its content height whenever it changes. A frame
 * sized to its content is taller than the pane, so a modal dialog would centre in the whole frame,
 * possibly off screen; each open modal is placed in the region the Shell reports as visible.
 */
export function startSettingsLayout(input: SettingsLayoutInput): SettingsLayout {
  const sheet = new CSSStyleSheet();
  sheet.replaceSync(CONTENT_SIZED_CSS);
  input.document.adoptedStyleSheets = [...input.document.adoptedStyleSheets, sheet];

  let lastHeight = -1;
  const report = (): void => {
    const height = Math.min(MAX_FRAME_EXTENT_PX, Math.ceil(input.host.getBoundingClientRect().height));
    if (height === lastHeight) return;
    lastHeight = height;
    input.reportHeight(height);
  };
  const resize = new ResizeObserver(report);
  resize.observe(input.host);

  let region: VisibleRegion | null = null;
  const placed = new Set<HTMLDialogElement>();
  const place = (): void => {
    for (const dialog of [...placed]) {
      if (!dialog.isConnected || !dialog.open) { release(dialog); placed.delete(dialog); }
    }
    if (region === null) return;
    for (const dialog of input.root.querySelectorAll("dialog")) {
      if (!dialog.matches(":modal")) continue;
      const room = Math.max(0, region.height - 2 * DIALOG_GUTTER_PX);
      dialog.style.setProperty("max-height", `${room}px`, "important");
      dialog.style.setProperty("margin-block", "0", "important");
      dialog.style.setProperty("bottom", "auto", "important");
      const offset = Math.max(DIALOG_GUTTER_PX, (region.height - dialog.offsetHeight) / 2);
      dialog.style.setProperty("top", `${Math.round(region.top + offset)}px`, "important");
      placed.add(dialog);
    }
  };
  const mutations = new MutationObserver(place);
  mutations.observe(input.root, { subtree: true, childList: true, attributes: true, attributeFilter: ["open"] });

  report();
  return Object.freeze({
    setVisibleRegion(next: VisibleRegion): void {
      region = next;
      place();
    },
    stop(): void {
      resize.disconnect();
      mutations.disconnect();
      for (const dialog of placed) release(dialog);
      placed.clear();
      input.document.adoptedStyleSheets = input.document.adoptedStyleSheets.filter((entry) => entry !== sheet);
    },
  });
}

function release(dialog: HTMLDialogElement): void {
  for (const property of ["max-height", "margin-block", "bottom", "top"]) dialog.style.removeProperty(property);
}
