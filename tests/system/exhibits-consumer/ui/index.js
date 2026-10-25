// Package UI of the test-only Exhibits fixture consumer. Hand written, no build step; it never
// ships. It drives the Consumer Backend's UI operations for the packaged smoke and for manual
// checks in a dev profile: each button publishes one fixture through the real Exhibits Ports,
// and the outcome is shown as visible text that Accessibility can read.
//
// "Open unsandboxed frame probe" frames this Package's own content view of the isolation probe
// in an iframe created WITHOUT a `sandbox` attribute. This UI may nest content only because the
// Consumer declares `teamarness.content-views`; the probe must still report an opaque origin,
// because the content scheme's CSP carries `sandbox allow-scripts`. The production Exhibits UI
// never creates such a frame.

const PUBLISH = [
  ["Publish revision 1", { fixture: "revision-1" }],
  ["Publish revision 2", { fixture: "revision-2", exhibitId: "fixture-revision-1" }],
  ["Publish media", { fixture: "media" }],
  ["Publish second Exhibit", { fixture: "revision-1", exhibitId: "fixture-second", title: "Second Exhibit fixture" }],
  ["Publish isolation probe", { fixture: "isolation-probe" }],
];

function codeOf(error) {
  if (typeof error !== "object" || error === null) return "unknown";
  return error.packageError?.details?.code ?? error.packageError?.code ?? error.code ?? "unknown";
}

export async function mount(context) {
  const container = document.createElement("div");
  const heading = document.createElement("h1");
  heading.textContent = "Exhibits fixture consumer";
  const controls = document.createElement("div");
  const log = document.createElement("ul");
  log.setAttribute("aria-label", "Fixture log");
  const frameSlot = document.createElement("div");

  const record = (text) => {
    const item = document.createElement("li");
    item.textContent = text;
    log.append(item);
  };

  const button = (label, action) => {
    const element = document.createElement("button");
    element.type = "button";
    element.textContent = label;
    element.addEventListener("click", async () => {
      element.disabled = true;
      try {
        await action();
      } catch (error) {
        record(`${label} failed: ${codeOf(error)}`);
      } finally {
        element.disabled = false;
      }
    });
    controls.append(element);
  };

  for (const [label, request] of PUBLISH) {
    button(label, async () => {
      const published = await context.client.request("fixture.publish", request);
      record(`Published ${published.exhibitId} revision ${published.revision} (${published.chunksSent} chunks)`);
    });
  }
  button("Open unsandboxed frame probe", async () => {
    const { url } = await context.client.request("frame.probe", {});
    const frame = document.createElement("iframe");
    // Deliberately no `sandbox` attribute: this frame checks the content scheme's own guarantee.
    frame.setAttribute("title", "Unsandboxed content frame");
    frame.style.width = "100%";
    frame.style.height = "360px";
    frame.src = `${url}#unsandboxed`;
    frameSlot.replaceChildren(frame);
    record("Opened unsandboxed frame probe");
  });

  container.append(heading, controls, log, frameSlot);
  context.root.append(container);

  let removed = false;
  return () => {
    if (removed) return;
    removed = true;
    container.remove();
  };
}
