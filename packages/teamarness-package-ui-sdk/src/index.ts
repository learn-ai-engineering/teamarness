export const TEAMARNESS_THEME_TOKENS = Object.freeze([
  "--teamarness-color-background",
  "--teamarness-color-panel",
  "--teamarness-color-text",
  "--teamarness-color-text-muted",
  "--teamarness-color-border",
  "--teamarness-color-accent",
  "--teamarness-color-danger",
  "--teamarness-color-focus",
  "--teamarness-font-body",
  "--teamarness-font-mono",
  "--teamarness-radius-control",
  "--teamarness-radius-panel",
] as const);

export type TeamarnessThemeToken = (typeof TEAMARNESS_THEME_TOKENS)[number];

const PACKAGE_UI_KIT_CSS = `
  :host, :host * { box-sizing: border-box; }

  /* Inherit the complete surrounding font instead of selecting var(--teamarness-font-body). */
  .teamarness-control {
    border: 1px solid var(--teamarness-color-border);
    border-radius: var(--teamarness-radius-control);
    background: var(--teamarness-color-panel);
    color: var(--teamarness-color-text);
    font: inherit;
    padding: 0.55rem 0.8rem;
  }
  .teamarness-control--primary {
    border-color: var(--teamarness-color-accent);
    background: var(--teamarness-color-accent);
    color: var(--teamarness-color-background);
  }
  .teamarness-control--secondary {
    border-color: var(--teamarness-color-border);
    background: var(--teamarness-color-panel);
    color: var(--teamarness-color-text);
  }
  .teamarness-field { display: grid; gap: 0.35rem; color: var(--teamarness-color-text); }
  .teamarness-input {
    width: 100%;
    border: 1px solid var(--teamarness-color-border);
    border-radius: var(--teamarness-radius-control);
    background: var(--teamarness-color-background);
    color: var(--teamarness-color-text);
    font: inherit;
    padding: 0.55rem 0.65rem;
  }
  .teamarness-help { color: var(--teamarness-color-text-muted); }
  .teamarness-validation { color: var(--teamarness-color-danger); font-family: var(--teamarness-font-mono); }
  .teamarness-notice {
    border: 1px solid var(--teamarness-color-border);
    border-radius: var(--teamarness-radius-control);
    background: var(--teamarness-color-panel);
    color: var(--teamarness-color-text);
    padding: 0.75rem;
  }
  .teamarness-notice--danger { border-color: var(--teamarness-color-danger); color: var(--teamarness-color-danger); }
  .teamarness-panel {
    border: 1px solid var(--teamarness-color-border);
    border-radius: var(--teamarness-radius-panel);
    background: var(--teamarness-color-panel);
    color: var(--teamarness-color-text);
    padding: 1rem;
  }
  .teamarness-stack { display: flex; flex-direction: column; gap: 0.75rem; }
  .teamarness-inline { display: flex; flex-wrap: wrap; align-items: center; gap: 0.75rem; }
  .teamarness-focus:focus-visible,
  .teamarness-control:focus-visible,
  .teamarness-input:focus-visible {
    outline: 3px solid var(--teamarness-color-focus);
    outline-offset: 2px;
  }
`;

const installedKits = new WeakMap<ShadowRoot, { readonly sheet: CSSStyleSheet; users: number }>();
const installedSuppressors = new WeakMap<ShadowRoot, { readonly observer: MutationObserver }>();

/**
 * Attributes that suppress WebKit autocorrection / autocapitalisation /
 * spelling suggestions on macOS Safari / WKWebView.
 */
const SUPPRESS_ATTRS: Record<string, string> = {
  autocapitalize: "off",
  autocorrect: "off",
  spellcheck: "false",
  writingsuggestions: "false",
} as const;

const INPUT_SELECTORS = "input,textarea,[contenteditable]";

/**
 * Walk a node (or its subtree) and suppress autocorrect attributes on
 * every input, textarea, and contenteditable element.
 */
function suppressInputs(node: Node): void {
  // Use tagName checks so this works in environments without
  // global HTMLInputElement / HTMLTextAreaElement constructors.
  if (node.nodeType === 1 /* ELEMENT_NODE */) {
    const el = node as Element;
    const tag = el.tagName;
    if (tag === "INPUT" || tag === "TEXTAREA") {
      for (const [attr, value] of Object.entries(SUPPRESS_ATTRS)) {
        el.setAttribute(attr, value);
      }
    }
    for (const child of el.querySelectorAll<HTMLElement>(INPUT_SELECTORS)) {
      for (const [attr, value] of Object.entries(SUPPRESS_ATTRS)) {
        child.setAttribute(attr, value);
      }
    }
  }
}

/**
 * Start a MutationObserver that suppresses autocorrect on any
 * input/textarea/contenteditable added to or within this shadow root.
 */
function installInputSuppressor(root: ShadowRoot): () => void {
  if (installedSuppressors.has(root)) return () => undefined;
  if (typeof MutationObserver !== "function") {
    // Environment (e.g. test runner) lacks MutationObserver.
    // Suppress any elements already present and bail.
    suppressInputs(root);
    return () => undefined;
  }

  const observer = new MutationObserver((mutations) => {
    for (const mutation of mutations) {
      for (const node of mutation.addedNodes) {
        suppressInputs(node);
      }
    }
  });
  observer.observe(root, { childList: true, subtree: true });

  // Suppress any elements already in the root.
  suppressInputs(root);

  installedSuppressors.set(root, { observer });

  let disconnected = false;
  return () => {
    if (disconnected) return;
    disconnected = true;
    observer.disconnect();
    installedSuppressors.delete(root);
  };
}

function currentAdoptedSheets(root: ShadowRoot): readonly CSSStyleSheet[] {
  if (!("adoptedStyleSheets" in root) || root.adoptedStyleSheets === undefined) {
    throw new Error("The Package UI kit requires ShadowRoot.adoptedStyleSheets support");
  }
  return root.adoptedStyleSheets;
}

function createKitSheet(): CSSStyleSheet {
  const StyleSheet = globalThis.CSSStyleSheet;
  if (typeof StyleSheet !== "function") {
    throw new Error("The Package UI kit requires constructed CSSStyleSheet support");
  }
  let sheet: CSSStyleSheet;
  try {
    sheet = new StyleSheet();
  } catch (cause) {
    throw new Error("The Package UI kit requires constructed CSSStyleSheet support", { cause });
  }
  if (typeof sheet.replaceSync !== "function") {
    throw new Error("The Package UI kit requires CSSStyleSheet.replaceSync support");
  }
  sheet.replaceSync(PACKAGE_UI_KIT_CSS);
  return sheet;
}

function setAdoptedSheets(root: ShadowRoot, sheets: CSSStyleSheet[]): void {
  try {
    root.adoptedStyleSheets = sheets;
  } catch (cause) {
    throw new Error("The Package UI kit requires mutable ShadowRoot.adoptedStyleSheets support", { cause });
  }
}

export function installPackageUiKit(root: ShadowRoot): () => void {
  let installation = installedKits.get(root);
  if (installation === undefined) {
    const sheet = createKitSheet();
    setAdoptedSheets(root, [...currentAdoptedSheets(root), sheet]);
    if (!currentAdoptedSheets(root).includes(sheet)) {
      throw new Error("The Package UI kit requires mutable ShadowRoot.adoptedStyleSheets support");
    }
    installation = { sheet, users: 0 };
    installedKits.set(root, installation);
  }
  installation.users += 1;

  // Start the input suppressor alongside the kit.  It is torn down when
  // the last kit user calls their cleanup function.
  const uninstallSuppressor = installInputSuppressor(root);

  let cleaned = false;
  return () => {
    if (cleaned) return;
    cleaned = true;
    const current = installedKits.get(root);
    if (current !== installation) return;
    current.users -= 1;
    if (current.users === 0) {
      uninstallSuppressor();
      setAdoptedSheets(root, currentAdoptedSheets(root).filter((sheet) => sheet !== current.sheet));
      installedKits.delete(root);
    }
  };
}

export type PackageValue = null | boolean | number | string | readonly PackageValue[] | { readonly [key: string]: PackageValue };

export type PackagePresentation = Readonly<{
  identity: string;
  version: string;
  displayName: string;
  source: "bundled" | "local";
  trust: "application_bundled" | "trusted_native_code_v1";
  lifecycle: "active";
  processGeneration: number;
}>;
export type WorkspacePresentation = Readonly<{ id: string; name: string }>;

export interface PackageSubscription {
  close(): Promise<void>;
}

export interface PackageClient {
  request<TResponse extends PackageValue = PackageValue>(
    operation: string,
    payload: PackageValue,
    options?: Readonly<{ signal?: AbortSignal }>,
  ): Promise<TResponse>;
  subscribe<TEvent extends PackageValue = PackageValue>(
    operation: string,
    payload: PackageValue,
    listener: (event: TEvent) => void | Promise<void>,
  ): Promise<PackageSubscription>;
}

export type PackageMountContext = Readonly<{
  root: ShadowRoot;
  package: PackagePresentation;
  workspace: WorkspacePresentation;
  uiHostVersion: number;
  themeTokens: readonly TeamarnessThemeToken[];
  client: PackageClient;
}>;

export type PackageUnmount = () => void | Promise<void>;
export type PackageUiModule = Readonly<{
  mount(context: PackageMountContext): PackageUnmount | Promise<PackageUnmount>;
}>;
