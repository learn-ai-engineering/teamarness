import {
  installPackageUiKit,
  TEAMARNESS_THEME_TOKENS,
  type PackageMountContext,
  type PackageSubscription,
  type PackageUnmount,
} from "@teamarness/package-ui-sdk";
import referenceAsset from "./probe.svg?no-inline";

const REFERENCE_PACKAGE_CSS = `
  :host { color: var(--teamarness-color-text); font-family: var(--teamarness-font-body); }
  .reference-action { align-self: flex-start; }
  .reference-details { display: grid; grid-template-columns: max-content 1fr; gap: .4rem .8rem; }
  .reference-details dt { font-weight: 700; }
  .reference-details dd { margin: 0; overflow-wrap: anywhere; }
`;

type ReferenceSnapshot = Readonly<{
  counter: number;
  backgroundMarker: number;
  membershipVersion: string;
  signingVerified: boolean;
  processGeneration: string;
  packageVersion: string;
}>;

function installPackageStyles(root: ShadowRoot): () => void {
  const StyleSheet = globalThis.CSSStyleSheet;
  if (typeof StyleSheet !== "function") {
    throw new Error("The reference Package requires constructed CSSStyleSheet support");
  }
  if (!("adoptedStyleSheets" in root) || root.adoptedStyleSheets === undefined) {
    throw new Error("The reference Package requires ShadowRoot.adoptedStyleSheets support");
  }
  let sheet: CSSStyleSheet;
  try {
    sheet = new StyleSheet();
  } catch (cause) {
    throw new Error("The reference Package requires constructed CSSStyleSheet support", { cause });
  }
  if (typeof sheet.replaceSync !== "function") {
    throw new Error("The reference Package requires CSSStyleSheet.replaceSync support");
  }
  sheet.replaceSync(REFERENCE_PACKAGE_CSS);
  try {
    root.adoptedStyleSheets = [...root.adoptedStyleSheets, sheet];
  } catch (cause) {
    throw new Error("The reference Package requires mutable ShadowRoot.adoptedStyleSheets support", { cause });
  }
  if (!root.adoptedStyleSheets.includes(sheet)) {
    throw new Error("The reference Package requires mutable ShadowRoot.adoptedStyleSheets support");
  }

  let cleaned = false;
  return () => {
    if (cleaned) return;
    cleaned = true;
    root.adoptedStyleSheets = root.adoptedStyleSheets.filter((candidate) => candidate !== sheet);
  };
}

function packageUiKitIsReady(section: HTMLElement, control: HTMLElement): boolean {
  const layout = getComputedStyle(section);
  const controlStyle = getComputedStyle(control);
  const inheritedAccent = controlStyle.getPropertyValue("--teamarness-color-accent").trim();
  return layout.display === "flex"
    && layout.flexDirection === "column"
    && controlStyle.borderTopStyle === "solid"
    && inheritedAccent.length > 0
    && controlStyle.backgroundColor.length > 0
    && controlStyle.backgroundColor === controlStyle.borderTopColor;
}

export async function mount(context: PackageMountContext): Promise<PackageUnmount> {
  const cleanupKit = installPackageUiKit(context.root);
  let cleanupPackageStyles: () => void;
  try {
    cleanupPackageStyles = installPackageStyles(context.root);
  } catch (error) {
    cleanupKit();
    throw error;
  }
  const article = document.createElement("article");
  article.className = "teamarness-page teamarness-page--readable";
  const header = document.createElement("header");
  header.className = "teamarness-page-header";
  const heading = document.createElement("h1");
  heading.className = "teamarness-page-title";
  heading.textContent = context.package.displayName;
  const subtitle = document.createElement("p");
  subtitle.className = "teamarness-page-subtitle";
  subtitle.textContent = "Exercises the Package foundation contracts end to end.";
  header.append(heading, subtitle);
  const section = document.createElement("section");
  section.className = "teamarness-section teamarness-stack";
  const logo = document.createElement("img");
  logo.src = referenceAsset;
  logo.alt = "Reference Package";
  logo.width = 48;
  logo.height = 48;
  const details = document.createElement("dl");
  details.className = "reference-details";
  const values = new Map<string, HTMLElement>();
  const themeReady = context.themeTokens.length === TEAMARNESS_THEME_TOKENS.length
    && TEAMARNESS_THEME_TOKENS.every((token, index) => context.themeTokens[index] === token);
  const presentation: readonly (readonly [string, string])[] = [
    ["Package UI", "Ready"],
    ["Workspace", "Ready"],
    ["Theme contract", themeReady ? "Ready" : "Unavailable"],
    ["UI kit", "Checking"],
    ["Package Backend", "Loading"],
    ["Membership access", "Loading"],
    ["Counter", "0"],
    ["Background marker", "0"],
    ["Signing round trip", "Loading"],
  ];
  for (const [label, value] of presentation) {
    const term = document.createElement("dt");
    term.textContent = label;
    const description = document.createElement("dd");
    description.textContent = value;
    values.set(label, description);
    details.append(term, description);
  }
  const increment = document.createElement("button");
  increment.type = "button";
  increment.className = "reference-action teamarness-control teamarness-control--primary";
  increment.textContent = "Increment counter";
  section.append(logo, details, increment);
  article.append(header, section);
  context.root.append(article);
  const uiKitStatus = packageUiKitIsReady(section, increment) ? "Ready" : "Unavailable";
  values.get("UI kit")!.textContent = uiKitStatus;
  values.get("UI kit")!.setAttribute("aria-label", `UI kit ${uiKitStatus}`);

  const render = (snapshot: ReferenceSnapshot): void => {
    values.get("Package Backend")!.textContent = "Ready";
    values.get("Membership access")!.textContent = "Ready";
    values.get("Counter")!.textContent = String(snapshot.counter);
    values.get("Background marker")!.textContent = String(snapshot.backgroundMarker);
    values.get("Signing round trip")!.textContent = snapshot.signingVerified ? "Verified" : "Failed";
  };

  render(await context.client.request<ReferenceSnapshot>("reference.read", {}));
  const subscription: PackageSubscription = await context.client.subscribe<ReferenceSnapshot>(
    "reference.changes",
    {},
    render,
  );
  const incrementCounter = async (): Promise<void> => {
    render(await context.client.request<ReferenceSnapshot>("reference.increment", {}));
  };
  const onIncrement = (): void => { void incrementCounter(); };
  increment.addEventListener("click", onIncrement);

  let unmounted = false;
  return async () => {
    if (unmounted) return;
    unmounted = true;
    increment.removeEventListener("click", onIncrement);
    await subscription.close();
    article.remove();
    cleanupPackageStyles();
    cleanupKit();
  };
}
