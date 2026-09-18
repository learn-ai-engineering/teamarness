import {
  adoptPackageStyles,
  describePackageError,
  installPackageUiKit,
  TEAMARNESS_THEME_TOKENS,
  typedPackageClient,
  type PackageMountContext,
  type PackageSubscription,
  type PackageUnmount,
} from "@teamarness/package-ui-sdk";
import type { ReferenceContract, Snapshot } from "./contract.generated";
import referenceAsset from "./probe.svg?no-inline";

const REFERENCE_PACKAGE_CSS = `
  :host { color: var(--teamarness-color-text); font-family: var(--teamarness-font-body); }
  .reference-heading { margin: 0; }
  .reference-action { align-self: flex-start; }
  .reference-details { display: grid; grid-template-columns: max-content 1fr; gap: .4rem .8rem; }
  .reference-details dt { font-weight: 700; }
  .reference-details dd { margin: 0; overflow-wrap: anywhere; }
`;

function packageUiKitIsReady(article: HTMLElement, control: HTMLElement): boolean {
  const layout = getComputedStyle(article);
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
  const client = typedPackageClient<ReferenceContract>(context.client);
  const cleanupKit = installPackageUiKit(context.root);
  let cleanupPackageStyles: () => void;
  try {
    cleanupPackageStyles = adoptPackageStyles(context.root, REFERENCE_PACKAGE_CSS);
  } catch (error) {
    cleanupKit();
    throw error;
  }
  const article = document.createElement("article");
  article.className = "teamarness-panel teamarness-stack";
  const heading = document.createElement("h2");
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
    ["UI kit", "Checking"],
    ["Package Backend", "Loading"],
    ["Membership access", "Loading"],
    ["Counter", "0"],
    ["Background marker", "0"],
    ["Signing round trip", "Loading"],
    ["Last Package error", "None"],
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
  // The live fixture for Package-owned error fidelity: a declared request the Backend always
  // rejects with its own code, so the whole path is demonstrable in the running application.
  const trigger = document.createElement("button");
  trigger.type = "button";
  trigger.className = "reference-action teamarness-control teamarness-control--secondary";
  trigger.textContent = "Trigger Package error";
  article.append(heading, logo, details, increment, trigger);
  context.root.append(article);
  const uiKitStatus = packageUiKitIsReady(article, increment) ? "Ready" : "Unavailable";
  values.get("UI kit")!.textContent = uiKitStatus;
  values.get("UI kit")!.setAttribute("aria-label", `UI kit ${uiKitStatus}`);

  const render = (snapshot: Snapshot): void => {
    values.get("Package Backend")!.textContent = "Ready";
    values.get("Membership access")!.textContent = "Ready";
    values.get("Counter")!.textContent = String(snapshot.counter);
    values.get("Background marker")!.textContent = String(snapshot.backgroundMarker);
    values.get("Signing round trip")!.textContent = snapshot.signingVerified ? "Verified" : "Failed";
  };

  render(await client.request("reference.read", {}));
  const subscription: PackageSubscription = await client.subscribe(
    "reference.changes",
    {},
    render,
  );
  const incrementCounter = async (): Promise<void> => {
    render(await client.request("reference.increment", {}));
  };
  const onIncrement = (): void => { void incrementCounter(); };
  increment.addEventListener("click", onIncrement);

  const triggerFailure = async (): Promise<void> => {
    try {
      await client.request("reference.fail", {});
    } catch (error) {
      const text = `Package error: ${describePackageError(error)}`;
      const value = values.get("Last Package error")!;
      value.textContent = text;
      value.setAttribute("aria-label", text);
    }
  };
  const onTrigger = (): void => { void triggerFailure(); };
  trigger.addEventListener("click", onTrigger);

  let unmounted = false;
  return async () => {
    if (unmounted) return;
    unmounted = true;
    increment.removeEventListener("click", onIncrement);
    trigger.removeEventListener("click", onTrigger);
    await subscription.close();
    article.remove();
    cleanupPackageStyles();
    cleanupKit();
  };
}
