// Exhibits fixture page 2: relative assets after a link navigation, then back to page 1.
import { check, imageLoaded, show } from "./common.mjs";

const checks = [];
checks.push(await check("page-2-loaded-by-link", () => location.pathname.endsWith("/pages/page2.html")));
checks.push(await check("page-2-parent-relative-stylesheet", () => getComputedStyle(document.getElementById("css-probe")).width === "42px"));
checks.push(await check("page-2-parent-relative-image", () => imageLoaded(document.getElementById("png-probe"))));
const passed = show(checks);
setTimeout(() => { location.href = `../index.html#page2=${passed ? "pass" : "fail"}`; }, 300);
