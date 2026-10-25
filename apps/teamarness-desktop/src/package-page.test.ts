import { describe, expect, it, vi } from "vitest";
import type {
  PackageAuthoringEntry,
  PackageHostSnapshot,
  PackageInstance,
  PackageInstanceKey,
  PackageManagementBridge,
  PackageRegistration,
} from "./package-contracts";
import { PackageManagementController, REMOVE_KEEPS_DATA_COPY } from "./package-install";
import { buildPackagePagePresentation, PackagePageController, type PackagePageTarget } from "./package-page";

const WORKSPACE = "00112233445566778899aabbccddeeff";
const OTHER_WORKSPACE = "ffeeddccbbaa99887766554433221100";
const TEAM = "dev.teamarness.conversations";
const AGENT = "com.example.agent";
const BARE = "com.example.bare";
const BROKEN = "com.example.broken";
const TUTORIAL = "dev.teamarness.tutorial";

const agentRegistration: PackageRegistration = {
  directory: "/Users/person/Packages/agent",
  expectedIdentity: AGENT,
  override: false,
  tier: "personal",
  status: { kind: "admitted" },
};

function instance(packageIdentity: string, lifecycle: PackageInstance["lifecycle"], workspaceId = WORKSPACE): PackageInstance {
  return { key: { packageIdentity, workspaceId }, lifecycle, processGeneration: 3, mountState: null };
}

function snapshot(change: Partial<PackageHostSnapshot> = {}): PackageHostSnapshot {
  return {
    revision: 7,
    state: "running",
    packages: [
      {
        identity: TEAM,
        version: "1.4.0",
        displayName: "Conversations",
        description: "Talk with your team.",
        author: "Teamarness",
        homepage: "https://teamarness.dev/conversations",
        source: "bundled",
        trust: "application_bundled",
        uiContractDigest: "digest",
      },
      {
        identity: AGENT,
        version: "0.3.0",
        displayName: "Agent",
        description: "Answers questions.",
        author: "Example Author",
        homepage: "https://example.com/agent",
        source: "local",
        trust: "trusted_native_code_v1",
        uiContractDigest: "digest",
      },
      {
        identity: BARE,
        version: "0.1.0",
        displayName: "Bare",
        source: "local",
        trust: "trusted_native_code_v1",
        uiContractDigest: "digest",
      },
    ],
    instances: [
      instance(TEAM, "active"),
      instance(AGENT, "active"),
      instance(AGENT, "suspended", OTHER_WORKSPACE),
      instance(BARE, "admitted"),
    ],
    diagnostics: [{
      version: 1,
      namespace: "teamarness.package-host",
      code: "port_missing",
      stage: "admission",
      affectedPackage: AGENT,
      affectedInstance: null,
      retryable: false,
      recovery: "none",
      diagnosticId: "d1",
      packageError: null,
    }],
    registrations: [
      agentRegistration,
      { ...agentRegistration, directory: "/Users/person/Packages/bare", expectedIdentity: BARE },
    ],
    ...change,
  };
}

const catalog: readonly PackageAuthoringEntry[] = [
  {
    identity: AGENT,
    capabilities: [{ identity: "teamarness.workspace-context", major: 1, minMinor: 0, maxMinor: 0 }],
    provided: [{ identity: "com.example.answers", major: 1, minor: 2, description: "Answers for other Packages" }],
    consumed: [
      { identity: "dev.teamarness.conversations", major: 1, minMinor: 0, maxMinor: 1, required: true, provider: TEAM, minor: 1, unavailable: null },
      { identity: "com.example.search", major: 2, minMinor: 0, maxMinor: 0, required: false, provider: null, minor: null, unavailable: "port_missing" },
    ],
  },
  {
    identity: TEAM,
    capabilities: [{ identity: "teamarness.package-ports", major: 1, minMinor: 0, maxMinor: 0 }],
    provided: [{ identity: "dev.teamarness.conversations", major: 1, minor: 1, description: "Conversations" }],
    consumed: [],
  },
  { identity: BARE, capabilities: [], provided: [], consumed: [] },
];

type Behavior = Partial<Pick<PackageManagementBridge, "authoringCatalog" | "restartPackage" | "remove" | "reload">>;

function harness(current: PackageHostSnapshot = snapshot(), behavior: Behavior = {}) {
  const calls: string[] = [];
  let latest = current;
  const removed: PackagePageTarget[] = [];
  const bridge: PackageManagementBridge = {
    chooseDirectory: async () => null,
    inspectDirectory: async () => { throw new Error("unused"); },
    install: async () => { throw new Error("unused"); },
    reload: behavior.reload ?? (async (identity) => { calls.push(`reload:${identity}`); return { revision: 8, restartRequired: false }; }),
    remove: behavior.remove ?? (async (identity) => { calls.push(`remove:${identity}`); return { revision: 8, restartRequired: false }; }),
    writeOverride: async () => { throw new Error("unused"); },
    restart: async () => { calls.push("app-restart"); },
    restartPackage: behavior.restartPackage ?? (async (key) => { calls.push(`restart:${key.packageIdentity}:${key.workspaceId}`); }),
    authoringCatalog: behavior.authoringCatalog ?? (async () => { calls.push("catalog"); return catalog; }),
  };
  const hooks = {
    changed: () => undefined,
    refresh: async () => { calls.push("refresh"); },
  };
  const management = new PackageManagementController(bridge, hooks);
  const lifecycle = (name: string) => async (key: PackageInstanceKey) => { calls.push(`${name}:${key.packageIdentity}:${key.workspaceId}`); };
  const page = new PackagePageController(bridge, {
    suspend: lifecycle("suspend"),
    recover: lifecycle("recover"),
    dispose: lifecycle("dispose"),
  }, management, {
    ...hooks,
    removed: (target) => { removed.push(target); },
    copy: async (value) => { calls.push(`copy:${value}`); },
  });
  return {
    page,
    calls,
    removed,
    setSnapshot: (value: PackageHostSnapshot) => { latest = value; },
    open: async (packageIdentity: string) => {
      page.open({ workspaceId: WORKSPACE, packageIdentity }, latest);
      await vi.waitFor(() => expect(page.render(latest)).not.toContain("Loading capabilities and Ports"));
    },
    html: () => page.render(latest),
    act: (action: string) => page.action(action, latest),
  };
}

const actions = (html: string): string[] =>
  [...html.matchAll(/data-package-page-action="([a-z-]+)"/g)].map(([, action]) => action!);

describe("Package page", () => {
  it("renders a Team Package's About, trust, Ports, and Instance state without Personal controls", async () => {
    const test = harness();
    await test.open(TEAM);
    const html = test.html();

    expect(html).toContain('<h1 id="package-page-heading" tabindex="-1" data-package-page-focus>Conversations</h1>');
    expect(html).toContain("Team Package");
    expect(html).toContain("<dt>Version</dt><dd>1.4.0</dd>");
    expect(html).toContain("<dt>Author</dt><dd>Teamarness</dd>");
    expect(html).toContain('value="https://teamarness.dev/conversations"');
    expect(html).toContain("<dt>Source</dt><dd>Team</dd>");
    expect(html).toContain("<dt>Trust</dt><dd>Bundled with Teamarness</dd>");
    expect(html).not.toContain("Package folder");
    expect(html).toContain("teamarness.package-ports 1.0");
    expect(html).toContain("dev.teamarness.conversations 1.1 — Conversations");
    expect(html).toContain("<dt>Availability</dt><dd>Available</dd>");
    expect(actions(html)).toEqual(["copy-homepage"]);
  });

  it("offers only the existing recovery controls on an unhealthy Team Package", async () => {
    const failed = snapshot({ instances: [instance(TEAM, "failed")] });
    const test = harness(failed);
    await test.open(TEAM);
    expect(actions(test.html())).toEqual(["copy-homepage", "recover", "dispose"]);
    for (const action of ["reload", "restart", "pause", "remove"]) await test.act(action);
    expect(test.calls.filter((call) => call !== "catalog")).toEqual([]);

    await test.act("recover");
    expect(test.calls.at(-1)).toBe(`recover:${TEAM}:${WORKSPACE}`);
  });

  it("renders a Personal Package with its folder, trust, Ports availability, diagnostics, and controls", async () => {
    const test = harness();
    await test.open(AGENT);
    const html = test.html();

    expect(html).toContain("Personal Package");
    expect(html).toContain("<dt>Source</dt><dd>Personal</dd>");
    expect(html).toContain("<dt>Package folder</dt><dd>/Users/person/Packages/agent</dd>");
    expect(html).toContain("Native code you trusted on this device.");
    expect(html).toContain("teamarness.workspace-context 1.0");
    expect(html).toContain("com.example.answers 1.2 — Answers for other Packages");
    expect(html).toContain("dev.teamarness.conversations 1.0–1.1<br><small>Available from Conversations, version 1.1</small>");
    expect(html).toContain("com.example.search 2.0 (optional)<br><small>Unavailable: A Port this Package requires is not available.</small>");
    expect(html).toContain("<h3>Diagnostics</h3>");
    expect(html).toContain("<dt>Process generation</dt><dd>3</dd>");
    expect(actions(html)).toEqual(["copy-homepage", "reload", "pause", "restart", "remove"]);
    expect(html).toContain("its process keeps running until you Resume it");
  });

  it("renders a manifest without optional metadata", async () => {
    const test = harness();
    await test.open(BARE);
    const html = test.html();

    expect(html).toContain(">Bare</h1>");
    expect(html).not.toContain("<dt>Description</dt>");
    expect(html).not.toContain("<dt>Author</dt>");
    expect(html).not.toContain("Homepage");
    expect(html).toContain("Semantic Capabilities</h3><p class=\"empty-state\">None declared.</p>");
    expect(actions(html)).toEqual(["reload", "start", "remove"]);
  });

  it("renders a refused registration by its identity, with the refusal, Reload, and Remove", async () => {
    const refused = snapshot({
      registrations: [{ ...agentRegistration, directory: "/Users/person/Packages/broken", expectedIdentity: BROKEN, status: { kind: "refused", code: "artifact_changed" } }],
    });
    const test = harness(refused);
    await test.open(BROKEN);
    const html = test.html();

    expect(html).toContain(`>${BROKEN}</h1>`);
    expect(html).toContain("It could not start: A file changed after the manifest was written. Rebuild the Package.");
    expect(html).toContain("<dt>Package folder</dt><dd>/Users/person/Packages/broken</dd>");
    expect(html).toContain("This Package is not running in this Workspace.");
    expect(html).toContain("Not available, because this Package is not admitted.");
    expect(actions(html)).toEqual(["reload", "remove"]);
  });

  it("escapes hostile metadata everywhere", async () => {
    const hostile = '<img src=x onerror="alert(1)">';
    const evil = snapshot();
    const test = harness({
      ...evil,
      packages: evil.packages.map((entry) => entry.identity === AGENT
        ? { ...entry, displayName: hostile, description: hostile, author: hostile, homepage: `" onfocus="alert(1)` }
        : entry),
      registrations: [{ ...agentRegistration, directory: `/tmp/${hostile}` }],
    }, {
      authoringCatalog: async () => [{
        identity: AGENT,
        capabilities: [{ identity: hostile, major: 1, minMinor: 0, maxMinor: 0 }],
        provided: [{ identity: hostile, major: 1, minor: 0, description: hostile }],
        consumed: [{ identity: hostile, major: 1, minMinor: 0, maxMinor: 0, required: true, provider: hostile, minor: 0, unavailable: null }],
      }],
    });
    await test.open(AGENT);
    const html = test.html();

    expect(html).not.toContain("<img");
    expect(html).not.toContain('" onfocus="');
    expect(html).toContain("&lt;img src=x onerror=&quot;alert(1)&quot;&gt;");
  });

  it("asks before Remove, says the data is kept, removes, and leaves the page", async () => {
    const test = harness();
    await test.open(AGENT);

    await test.act("remove");
    const confirming = test.html();
    expect(confirming).toContain(REMOVE_KEEPS_DATA_COPY);
    expect(confirming).toContain("It stops in all your Workspaces.");
    expect(actions(confirming)).toEqual(["copy-homepage", "confirm-remove", "cancel-remove"]);
    expect(test.calls).not.toContain(`remove:${AGENT}`);

    await test.act("confirm-remove");
    expect(test.calls).toContain(`remove:${AGENT}`);
    expect(test.removed).toEqual([{ workspaceId: WORKSPACE, packageIdentity: AGENT }]);
  });

  it("keeps the page and offers a restart after an override is removed", async () => {
    const overridden = snapshot({
      registrations: [{ directory: "/Users/person/Packages/tutorial", expectedIdentity: TUTORIAL, override: true, tier: "team", status: { kind: "admitted" } }],
      packages: [...snapshot().packages, {
        identity: TUTORIAL, version: "9.0.0", displayName: "Tutorial", source: "local", trust: "trusted_native_code_v1", uiContractDigest: "digest",
      }],
      instances: [instance(TUTORIAL, "active")],
    });
    const test = harness(overridden, {
      remove: async (identity) => { test.calls.push(`remove:${identity}`); return { revision: null, restartRequired: true }; },
    });
    await test.open(TUTORIAL);
    expect(test.html()).toContain("<dt>Source</dt><dd>Team, replaced on this device by an override</dd>");
    expect(actions(test.html())).toEqual(["remove"]);

    await test.act("remove");
    expect(test.html()).toContain("Remove the override? Tutorial returns to the Team Package after Teamarness restarts. Data is kept.");
    await test.act("confirm-remove");
    expect(test.removed).toEqual([]);
    expect(test.html()).toContain("Restart Teamarness to use the Team Package again.");
    await test.act("restart-app");
    expect(test.calls.at(-1)).toBe("app-restart");
  });

  it("calls the right commands for Reload, Restart, Pause, Resume, and Start", async () => {
    const test = harness();
    await test.open(AGENT);

    await test.act("reload");
    expect(test.calls).toContain(`reload:${AGENT}`);
    await test.act("restart");
    expect(test.calls.slice(-2)).toEqual([`restart:${AGENT}:${WORKSPACE}`, "refresh"]);
    expect(test.html()).toContain("Restarted Agent in this Workspace.");
    await test.act("pause");
    expect(test.calls.at(-1)).toBe(`suspend:${AGENT}:${WORKSPACE}`);
    expect(test.html()).toContain("Paused Agent.");

    test.setSnapshot(snapshot({ instances: [instance(AGENT, "suspended")] }));
    expect(actions(test.html())).toEqual(["copy-homepage", "reload", "resume", "remove"]);
    await test.act("resume");
    expect(test.calls.at(-1)).toBe(`recover:${AGENT}:${WORKSPACE}`);

    test.setSnapshot(snapshot({ instances: [instance(AGENT, "admitted")] }));
    await test.act("start");
    expect(test.calls.at(-1)).toBe(`recover:${AGENT}:${WORKSPACE}`);
    // A control the current state does not offer does nothing.
    await test.act("pause");
    expect(test.calls.at(-1)).toBe(`recover:${AGENT}:${WORKSPACE}`);
  });

  it("names the rule when Restart is refused", async () => {
    const test = harness(snapshot(), {
      restartPackage: async () => { throw { code: "invalid_lifecycle_state" }; },
    });
    await test.open(AGENT);
    await test.act("restart");
    expect(test.html()).toContain('role="alert"><p>This Package cannot do that in its current state.');
  });

  it("copies the homepage as text", async () => {
    const test = harness();
    await test.open(AGENT);
    await test.act("copy-homepage");
    expect(test.calls.at(-1)).toBe("copy:https://example.com/agent");
    expect(test.html()).toContain("Homepage copied.");
  });

  it("shows a catalog failure with Retry and reads the catalog again for a new revision", async () => {
    let fail = true;
    const test = harness(snapshot(), {
      authoringCatalog: async () => {
        test.calls.push("catalog");
        if (fail) throw new Error("unavailable");
        return catalog;
      },
    });
    await test.open(AGENT);
    expect(test.html()).toContain("Capabilities and Ports could not be read.");
    fail = false;
    await test.act("retry-catalog");
    expect(test.html()).toContain("com.example.answers 1.2");

    const reads = test.calls.filter((call) => call === "catalog").length;
    test.page.snapshotChanged(snapshot());
    test.page.snapshotChanged(snapshot({ revision: 8 }));
    expect(test.calls.filter((call) => call === "catalog").length).toBe(reads + 1);
  });

  it("returns nothing to show for a Package this device does not have", () => {
    expect(buildPackagePagePresentation(snapshot(), { workspaceId: WORKSPACE, packageIdentity: "com.example.gone" })).toBeNull();
    const test = harness();
    test.page.open({ workspaceId: WORKSPACE, packageIdentity: "com.example.gone" }, snapshot());
    expect(test.html()).toContain("This Package is no longer installed on this device.");
    expect(test.html()).toContain("data-package-page-back");
  });
});
