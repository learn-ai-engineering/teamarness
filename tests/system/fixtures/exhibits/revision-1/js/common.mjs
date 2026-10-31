// Shared check helpers for the Exhibits fixture. Results are shown as visible text only: the
// fixture never posts to its parent, because the Exhibits UI trusts no message from an Exhibit.
export async function check(name, run) {
  try { return [name, (await run()) === true]; } catch { return [name, false]; }
}
export function imageLoaded(element) {
  if (element.complete) return Promise.resolve(element.naturalWidth > 0);
  return new Promise((resolve) => {
    element.addEventListener("load", () => resolve(element.naturalWidth > 0), { once: true });
    element.addEventListener("error", () => resolve(false), { once: true });
  });
}
export function show(checks, prefix = "") {
  const list = document.getElementById("checks");
  for (const [name, passed] of checks) {
    const item = document.createElement("li");
    item.textContent = `${passed ? "PASS" : "FAIL"} ${name}`;
    list.append(item);
  }
  const passed = checks.every(([, ok]) => ok);
  document.getElementById("verdict").textContent = `${prefix}${passed ? "PASS" : "FAIL"}`;
  return passed;
}
