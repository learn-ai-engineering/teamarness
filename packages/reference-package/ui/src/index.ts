import type {
  PackageMountContext,
  PackageSubscription,
  PackageUnmount,
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
  const style = document.createElement("style");
  style.textContent = `
    :host { color: var(--teamarness-color-text); font-family: var(--teamarness-font-body); }
    article { background: var(--teamarness-color-panel); border-radius: var(--teamarness-radius-control); padding: 1rem; }
    dl { display: grid; grid-template-columns: max-content 1fr; gap: .4rem .8rem; }
    dt { font-weight: 700; }
    dd { margin: 0; overflow-wrap: anywhere; }
    button { background: var(--teamarness-color-accent); border: 0; border-radius: var(--teamarness-radius-control); padding: .55rem .8rem; }
  `;
  const article = document.createElement("article");
  const heading = document.createElement("h3");
  heading.textContent = context.package.displayName;
  const logo = document.createElement("img");
  logo.src = referenceAsset;
  logo.alt = "Reference Package";
  logo.width = 48;
  logo.height = 48;
  const details = document.createElement("dl");
  const values = new Map<string, HTMLElement>();
  const presentation: readonly (readonly [string, string])[] = [
    ["Package", `${context.package.identity} ${context.package.version}`],
    ["Workspace", `${context.workspace.name} (${context.workspace.id})`],
    ["Source", context.package.source],
    ["Trust", context.package.trust],
    ["Lifecycle", context.package.lifecycle],
    ["Process generation", String(context.package.processGeneration)],
    ["Counter", "0"],
    ["Background marker", "0"],
    ["Membership version", "Loading"],
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
  increment.textContent = "Increment counter";
  article.append(heading, logo, details, increment);
  context.root.append(style, article);

  const render = (snapshot: ReferenceSnapshot): void => {
    values.get("Counter")!.textContent = String(snapshot.counter);
    values.get("Background marker")!.textContent = String(snapshot.backgroundMarker);
    values.get("Membership version")!.textContent = snapshot.membershipVersion;
    values.get("Signing round trip")!.textContent = snapshot.signingVerified ? "verified" : "failed";
    values.get("Process generation")!.textContent = snapshot.processGeneration;
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
  };
}
