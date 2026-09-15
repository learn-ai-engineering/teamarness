import {
  installPackageUiKit,
  TEAMARNESS_THEME_TOKENS,
  type PackageMountContext,
  type PackageSubscription,
  type PackageUnmount,
} from "@teamarness/package-ui-sdk";
import referenceAsset from "./probe.svg?no-inline";

type ReferenceSnapshot = Readonly<{
  counter: number;
  backgroundMarker: number;
  membershipVersion: string;
  signingVerified: boolean;
  processGeneration: string;
  packageVersion: string;
}>;

export async function mount(context: PackageMountContext): Promise<PackageUnmount> {
  const cleanupKit = installPackageUiKit(context.root);
  const style = document.createElement("style");
  style.textContent = `
    :host { color: var(--teamarness-color-text); font-family: var(--teamarness-font-body); }
    .reference-heading { margin: 0; }
    .reference-details { display: grid; grid-template-columns: max-content 1fr; gap: .4rem .8rem; }
    .reference-details dt { font-weight: 700; }
    .reference-details dd { margin: 0; overflow-wrap: anywhere; }
  `;
  const article = document.createElement("article");
  article.className = "teamarness-panel teamarness-stack";
  const heading = document.createElement("h3");
  heading.className = "reference-heading";
  heading.textContent = context.package.displayName;
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
  increment.className = "teamarness-control teamarness-control--primary";
  increment.textContent = "Increment counter";
  article.append(heading, logo, details, increment);
  context.root.append(style, article);

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
    style.remove();
    cleanupKit();
  };
}
