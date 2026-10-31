// The Shell's part in the frame scenarios: one sandboxed `allow-scripts` frame per view, and a
// relay between each frame's Package client and the harness Backends in Node
// (`window.__harness`, exposed by the scenario runner). Node delivers events with `__deliver`.
const frames = new Map();

window.__addView = (view, displayName = "Documents") => new Promise((resolve) => {
  const iframe = document.createElement("iframe");
  iframe.setAttribute("sandbox", "allow-scripts");
  iframe.title = view;
  iframe.width = "1100";
  iframe.height = "640";
  iframe.referrerPolicy = "no-referrer";
  frames.set(view, { iframe, displayName, mounted: resolve });
  iframe.src = `frame.html?view=${encodeURIComponent(view)}`;
  document.getElementById("frames").append(iframe);
});

window.__removeView = (view) => {
  frames.get(view)?.iframe.remove();
  frames.delete(view);
};

window.__deliver = (view, message) => {
  frames.get(view)?.iframe.contentWindow?.postMessage(message, "*");
};

window.addEventListener("message", async (event) => {
  const entry = [...frames.entries()].find(([, frame]) => frame.iframe.contentWindow === event.source);
  if (!entry) return;
  const [view, frame] = entry;
  const message = event.data;
  if (message.kind === "hello") {
    frame.iframe.contentWindow.postMessage({ kind: "mount", displayName: frame.displayName }, "*");
    return;
  }
  if (message.kind === "mounted" || message.kind === "fault") {
    frame.mounted(message);
    if (message.kind === "mounted") return;
  }
  const answer = await window.__harness(view, message);
  if (answer !== undefined && answer !== null) frame.iframe.contentWindow?.postMessage(answer, "*");
});
