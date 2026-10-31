// Exhibits fixture page 1: rendering, relative assets, CSS @import, modules, own-file fetch and
// WebAssembly. On first load it navigates to page 2, which navigates back with its verdict in
// the fragment; page 1 then shows the combined verdict and stays.
import { answer } from "./lib.mjs";
import { check, imageLoaded, show } from "./common.mjs";

const checks = [];
checks.push(await check("inline-script", () => window.__inlineScriptRan === true));
checks.push(await check("inline-style", () => getComputedStyle(document.getElementById("inline-style-probe")).width === "17px"));
checks.push(await check("relative-stylesheet", () => getComputedStyle(document.getElementById("css-probe")).width === "42px"));
checks.push(await check("css-import", () => getComputedStyle(document.getElementById("css-probe")).minWidth === "7px"));
checks.push(await check("png-image", () => imageLoaded(document.getElementById("png-probe"))));
checks.push(await check("svg-image", () => imageLoaded(document.getElementById("svg-probe"))));
checks.push(await check("inline-svg", () => document.getElementById("inline-svg").getBBox().width > 0));
checks.push(await check("module-static-import", () => answer === 42));
checks.push(await check("module-dynamic-import", async () => (await import("./dynamic.mjs")).dynamicValue === "loaded"));
checks.push(await check("fetch-own-json", async () => (await (await fetch(new URL("../assets/data.json", import.meta.url))).json()).ok === true));
checks.push(await check("wasm-instantiate-streaming", async () => {
  const { instance } = await WebAssembly.instantiateStreaming(fetch(new URL("../wasm/add.wasm", import.meta.url)));
  return instance.exports.add(2, 3) === 5;
}));
checks.push(await check("wasm-compile-bytes", async () => {
  const bytes = await (await fetch(new URL("../wasm/add.wasm", import.meta.url))).arrayBuffer();
  const { instance } = await WebAssembly.instantiate(bytes);
  return instance.exports.add(4, 5) === 9;
}));

const returned = /^#page2=(pass|fail)$/.exec(location.hash);
if (returned) {
  checks.push(["navigated-to-page-2-and-back", returned[1] === "pass"]);
  show(checks);
} else if (show(checks, "PAGE 1 ")) {
  setTimeout(() => document.getElementById("to-page2").click(), 300);
}
