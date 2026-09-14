import type { PackageMountContext, PackageUnmount } from "@teamarness/package-ui-sdk";
import probeAsset from "./probe.svg?no-inline";

const previousAsset = Symbol.for("teamarness.phase5.previous-admitted-asset");
type ProbeGlobal = typeof globalThis & { [previousAsset]?: string };

export async function mount(context: PackageMountContext): Promise<PackageUnmount> {
  const style = document.createElement("style");
  style.textContent = `
    :host { color: var(--teamarness-color-text); font-family: var(--teamarness-font-body); }
    article { background: var(--teamarness-color-background); border-radius: var(--teamarness-radius-control); padding: 1rem; }
    img { display: block; height: 3rem; width: 3rem; }
    li[data-result="pass"] { color: var(--teamarness-color-accent); }
    li[data-result="fail"] { color: var(--teamarness-color-danger); }
  `;
  const article = document.createElement("article");
  const heading = document.createElement("h3");
  heading.textContent = `${context.package.displayName} · ${context.workspace.name}`;
  const note = document.createElement("p");
  note.textContent = "Phase 5 WebKit resource checks";
  const image = document.createElement("img");
  image.alt = "Admitted Package asset";
  image.src = probeAsset;
  const results = document.createElement("ul");
  article.append(heading, note, image, results);
  context.root.append(style, article);

  const base = new URL(import.meta.url);
  const admittedAsset = new URL(probeAsset, base).href;
  const prior = (globalThis as ProbeGlobal)[previousAsset];
  (globalThis as ProbeGlobal)[previousAsset] = admittedAsset;

  await check(results, "Admitted ESM mounted in a fresh Shadow Root", async () => context.root.host !== null);
  await check(results, "Declared asset kept its admitted media type", async () => {
    const response = await fetch(admittedAsset);
    return response.ok && response.headers.get("content-type") === "image/svg+xml";
  });
  await check(results, "Traversal failed", async () => !(await fetch(new URL("../../not-admitted.txt", base))).ok);
  await check(results, "MIME confusion failed", async () => {
    try {
      await import(/* @vite-ignore */ admittedAsset);
      return false;
    } catch {
      return true;
    }
  });
  await check(results, "file: load failed", async () => rejects(fetch("file:///etc/hosts")));
  await check(results, "Remote connection failed", async () => rejects(fetch("https://example.invalid/phase-5")));
  await check(results, "Remote script failed", async () => remoteScriptFails(context.root));
  if (prior) {
    await check(results, "Stale admitted URL failed after reload", async () => {
      try {
        return !(await fetch(prior)).ok;
      } catch {
        return true;
      }
    });
  } else {
    appendResult(results, "Reload UI to run the stale URL check", true);
  }

  let unmounted = false;
  return async () => {
    if (unmounted) return;
    unmounted = true;
    article.remove();
    style.remove();
  };
}

async function check(results: HTMLUListElement, label: string, operation: () => Promise<boolean>): Promise<void> {
  try {
    appendResult(results, label, await operation());
  } catch {
    appendResult(results, label, false);
  }
}

function appendResult(results: HTMLUListElement, label: string, passed: boolean): void {
  const item = document.createElement("li");
  item.dataset.result = passed ? "pass" : "fail";
  item.textContent = `${passed ? "PASS" : "FAIL"}: ${label}`;
  results.append(item);
}

async function rejects(operation: Promise<unknown>): Promise<boolean> {
  try {
    await operation;
    return false;
  } catch {
    return true;
  }
}

function remoteScriptFails(root: ShadowRoot): Promise<boolean> {
  return new Promise((resolve) => {
    const script = document.createElement("script");
    script.src = "https://example.invalid/phase-5.js";
    script.onload = () => resolve(false);
    script.onerror = () => resolve(true);
    root.append(script);
  });
}
