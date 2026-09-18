import { runPackageFrame, type FrameDocument, type FrameWindow } from "./runtime";

runPackageFrame({
  window: window as unknown as FrameWindow,
  document: document as unknown as FrameDocument,
  runtimeUrl: import.meta.url,
  importModule: (url) => import(/* @vite-ignore */ url) as Promise<unknown>,
  createStyleSheet: () => new CSSStyleSheet(),
});
