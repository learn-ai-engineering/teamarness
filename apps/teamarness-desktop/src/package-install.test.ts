import { describe, expect, it } from "vitest";
import type {
  InspectedPackage,
  PackageChange,
  PackageHostSnapshot,
  PackageManagementBridge,
  PackageRegistration,
  PackageRegistrationInput,
} from "./package-contracts";
import {
  INSTALLED_COPY,
  PackageManagementController,
  packageRefusalMessage,
  TRUSTED_NATIVE_CODE_RISKS,
} from "./package-install";

const full: InspectedPackage = {
  directory: "/Users/person/Packages/agent",
  identity: "com.example.agent",
  version: "1.2.3",
  displayName: "Agent",
  description: "Answers questions in the Workspace.",
  author: "Example Author",
  homepage: "https://example.com/agent",
  capabilities: [{ identity: "teamarness.workspace-context", major: 1, minMinor: 0, maxMinor: 0 }],
  provided: [{ identity: "com.example.answers", major: 1, minor: 2, description: "Answers" }],
  consumed: [{ identity: "com.example.messages", major: 1, minMinor: 0, maxMinor: 0, required: false }],
  kind: "new",
};

const bare: InspectedPackage = {
  directory: "/Users/person/Packages/bare",
  identity: "com.example.bare",
  version: "0.1.0",
  displayName: "Bare",
  capabilities: [],
  provided: [],
  consumed: [],
  kind: "new",
};

const tutorialOverride: InspectedPackage = {
  ...bare,
  directory: "/Users/person/Packages/tutorial",
  identity: "dev.teamarness.tutorial",
  displayName: "Tutorial (local build)",
  kind: "overrides_team",
};

function snapshot(registrations: readonly PackageRegistration[] = []): PackageHostSnapshot {
  return {
    revision: 4,
    state: "running",
    packages: [{
      identity: "dev.teamarness.tutorial",
      version: "1.0.0",
      displayName: "Tutorial",
      source: "bundled",
      trust: "application_bundled",
      uiContractDigest: "digest",
    }],
    instances: [],
    diagnostics: [],
    registrations,
  };
}

type Call = readonly [string, unknown?];

function harness(behavior: Partial<PackageManagementBridge> & { inspected?: InspectedPackage } = {}) {
  const calls: Call[] = [];
  let renders = 0;
  let refreshes = 0;
  const live: PackageChange = { revision: 5, restartRequired: false };
  const bridge: PackageManagementBridge = {
    chooseDirectory: behavior.chooseDirectory ?? (async () => {
      calls.push(["choose"]);
      return (behavior.inspected ?? full).directory;
    }),
    inspectDirectory: behavior.inspectDirectory ?? (async (directory: string) => {
      calls.push(["inspect", directory]);
      return behavior.inspected ?? full;
    }),
    install: behavior.install ?? (async (input: PackageRegistrationInput) => { calls.push(["install", input]); return live; }),
    reload: behavior.reload ?? (async (identity: string) => { calls.push(["reload", identity]); return live; }),
    remove: behavior.remove ?? (async (identity: string) => { calls.push(["remove", identity]); return live; }),
    writeOverride: behavior.writeOverride ?? (async (input: PackageRegistrationInput) => {
      calls.push(["override", input]);
      return { revision: null, restartRequired: true };
    }),
    restart: behavior.restart ?? (async () => { calls.push(["restart"]); }),
    restartPackage: async () => { calls.push(["restart-package"]); },
    authoringCatalog: async () => [],
  };
  const controller = new PackageManagementController(bridge, {
    changed: () => { renders += 1; },
    refresh: async () => { refreshes += 1; },
  });
  return {
    controller,
    calls,
    names: () => calls.map(([name]) => name),
    renders: () => renders,
    refreshes: () => refreshes,
  };
}

describe("Add Personal Package flow", () => {
  it("previews every declared field and omits the ones a manifest leaves out", async () => {
    const test = harness();
    await test.controller.installAction("open");
    await test.controller.installAction("choose");
    const html = test.controller.renderPanel(snapshot());

    expect(test.controller.state().kind).toBe("preview");
    for (const value of [
      "Agent", "com.example.agent", "1.2.3", "Answers questions in the Workspace.", "Example Author",
      "https://example.com/agent", "/Users/person/Packages/agent", "teamarness.workspace-context 1",
      "com.example.answers 1.2", "com.example.messages 1 (optional)",
    ]) expect(html).toContain(value);
    expect(html).toContain("data-package-install-focus");

    const bareTest = harness({ inspected: bare });
    await bareTest.controller.installAction("open");
    await bareTest.controller.installAction("choose");
    const bareHtml = bareTest.controller.renderPanel(snapshot());
    expect(bareHtml).toContain("Bare");
    for (const term of ["Description", "Author", "Homepage"]) expect(bareHtml).not.toContain(`<dt>${term}</dt>`);
    expect(bareHtml).toContain("<dt>Capabilities</dt><dd>None</dd>");
  });

  it("does not install until the person trusts the Package, then installs with exactly its risks", async () => {
    const test = harness();
    await test.controller.installAction("open");
    // An action that does not belong to the current step is ignored.
    await test.controller.installAction("trust");
    await test.controller.installAction("choose");
    await test.controller.installAction("trust");
    expect(test.names()).toEqual(["choose", "inspect"]);

    await test.controller.installAction("continue");
    const consent = test.controller.renderPanel(snapshot());
    expect(consent).toContain("Trust and install");
    for (const risk of TRUSTED_NATIVE_CODE_RISKS) expect(consent.toLowerCase()).toContain(risk);
    expect(test.names()).not.toContain("install");

    await test.controller.installAction("trust");
    expect(test.calls.at(-1)).toEqual(["install", {
      directory: full.directory,
      expectedIdentity: full.identity,
      acceptedRisks: TRUSTED_NATIVE_CODE_RISKS,
    }]);
    expect(test.refreshes()).toBe(1);
    expect(test.controller.state()).toEqual({ kind: "installed", name: "Agent" });
    expect(test.controller.renderPanel(snapshot())).toContain(INSTALLED_COPY);
    expect(INSTALLED_COPY).toBe("Installed on this device. It appears in all your Workspaces.");

    await test.controller.installAction("done");
    expect(test.controller.renderPanel(snapshot())).toBe("");
  });

  it("never installs when the person cancels at consent", async () => {
    const test = harness();
    await test.controller.installAction("open");
    await test.controller.installAction("choose");
    await test.controller.installAction("continue");
    await test.controller.installAction("cancel");
    expect(test.controller.state().kind).toBe("closed");
    expect(test.names()).not.toContain("install");
  });

  it("shows inline progress while installing", async () => {
    let finish: (change: PackageChange) => void = () => undefined;
    const test = harness({ install: () => new Promise((resolve) => { finish = resolve; }) });
    await test.controller.installAction("open");
    await test.controller.installAction("choose");
    await test.controller.installAction("continue");
    const installing = test.controller.installAction("trust");
    const progress = test.controller.renderPanel(snapshot());
    expect(progress).toContain('role="status"');
    expect(progress).toContain("Installing Agent…");
    // Cancel cannot interrupt a write in progress.
    await test.controller.installAction("cancel");
    finish({ revision: 6, restartRequired: false });
    await installing;
    expect(test.controller.state().kind).toBe("installed");
  });

  it("asks for a second confirmation before writing an override, then offers a restart", async () => {
    const test = harness({ inspected: tutorialOverride });
    await test.controller.installAction("open");
    await test.controller.installAction("choose");
    const preview = test.controller.renderPanel(snapshot());
    expect(preview).toContain("same identity as the Team Package Tutorial");
    expect(preview).toContain("replaces Tutorial on this device after Teamarness restarts");

    await test.controller.installAction("continue");
    expect(test.controller.renderPanel(snapshot())).toContain("Trust and continue");
    await test.controller.installAction("trust");
    const confirmation = test.controller.renderPanel(snapshot());
    expect(test.controller.state().kind).toBe("override");
    expect(confirmation).toContain("second, separate confirmation");
    expect(confirmation).toContain("Replace Tutorial after restart");
    expect(test.names()).toEqual(["choose", "inspect"]);

    await test.controller.installAction("override");
    expect(test.calls.at(-1)).toEqual(["override", {
      directory: tutorialOverride.directory,
      expectedIdentity: tutorialOverride.identity,
      acceptedRisks: TRUSTED_NATIVE_CODE_RISKS,
    }]);
    expect(test.names()).not.toContain("install");
    const restart = test.controller.renderPanel(snapshot());
    expect(restart).toContain("Restart Teamarness");
    expect(restart).toContain("replaces the Team Package after Teamarness restarts");

    await test.controller.installAction("restart");
    expect(test.names().at(-1)).toBe("restart");
  });

  it("can stop before the override is written", async () => {
    const test = harness({ inspected: tutorialOverride });
    await test.controller.installAction("open");
    await test.controller.installAction("choose");
    await test.controller.installAction("continue");
    await test.controller.installAction("trust");
    await test.controller.installAction("cancel");
    expect(test.names()).toEqual(["choose", "inspect"]);
  });

  it("renders a readable refusal that names the failed rule and keeps the person in the flow", async () => {
    const test = harness({
      inspectDirectory: async () => { throw { code: "artifact_changed" }; },
    });
    await test.controller.installAction("open");
    await test.controller.installAction("choose");
    const html = test.controller.renderPanel(snapshot());
    expect(test.controller.state().kind).toBe("choose");
    expect(html).toContain('role="alert"');
    expect(html).toContain("A file changed after the manifest was written. Rebuild the Package.");
  });

  it("re-reads the snapshot after a refused install", async () => {
    const test = harness({ install: async () => { throw { code: "port_provider_conflict" }; } });
    await test.controller.installAction("open");
    await test.controller.installAction("choose");
    await test.controller.installAction("continue");
    await test.controller.installAction("trust");
    expect(test.refreshes()).toBe(1);
    expect(test.controller.renderPanel(snapshot())).toContain("Another Package already provides a Port contract");
  });

  it.each([
    ["manifest_malformed", "is missing, cannot be read, or breaks a manifest rule"],
    ["manifest_malformed", "Run teamarness-package-validate on the folder to see which rule."],
    ["source_not_found", "That folder does not exist."],
    ["invalid_registration", "full path"],
    ["override_required", "can only be replaced by an override"],
    ["registration_file_invalid", "will not change it"],
    ["port_contract_missing", "requires is not available"],
    ["ui_view_not_declared", "does not offer that view"],
  ])("maps %s to a readable message", (code, wording) => {
    expect(packageRefusalMessage(code)).toContain(wording);
  });

  it("still names an unknown rule", () => {
    expect(packageRefusalMessage("some_new_rule")).toContain("some_new_rule");
  });

  it("checks a pasted path before reading it", async () => {
    const test = harness();
    await test.controller.installAction("open");
    await test.controller.installAction("paste");
    expect(test.controller.renderPanel(snapshot())).toContain('<label for="package-install-path">Package folder path</label>');
    await test.controller.inspectPath("Packages/agent");
    expect(test.names()).toEqual([]);
    expect(test.controller.renderPanel(snapshot())).toContain("Enter the full path of the Package folder");

    await test.controller.inspectPath("  /Users/person/Packages/agent  ");
    expect(test.calls).toEqual([["inspect", "/Users/person/Packages/agent"]]);
    expect(test.controller.state().kind).toBe("preview");
  });

  it("stays on the chooser when the person closes the folder picker", async () => {
    const test = harness({ chooseDirectory: async () => null });
    await test.controller.installAction("open");
    await test.controller.installAction("choose");
    expect(test.controller.state()).toEqual({ kind: "choose", pasting: false, refusal: null });
  });

  it("requests focus on each new step", async () => {
    const test = harness();
    expect(test.controller.takeFocusRequest()).toBe(false);
    await test.controller.installAction("open");
    expect(test.controller.takeFocusRequest()).toBe(true);
    expect(test.controller.takeFocusRequest()).toBe(false);
  });

  it("escapes hostile Package metadata", async () => {
    const test = harness({
      inspected: { ...full, displayName: '<img src=x onerror="alert(1)">', description: "<script>bad()</script>" },
    });
    await test.controller.installAction("open");
    await test.controller.installAction("choose");
    const html = test.controller.renderPanel(snapshot());
    expect(html).not.toContain("<img");
    expect(html).not.toContain("<script>");
    expect(html).toContain("&lt;img src=x onerror=&quot;alert(1)&quot;&gt;");
  });
});

describe("registrations that need attention", () => {
  const refused: PackageRegistration = {
    directory: "/Users/person/Packages/broken",
    expectedIdentity: "com.example.broken",
    override: false,
    tier: "personal",
    status: { kind: "refused", code: "artifact_changed" },
  };
  const admitted: PackageRegistration = { ...refused, expectedIdentity: "com.example.fine", status: { kind: "admitted" } };
  const override: PackageRegistration = {
    directory: "/Users/person/Packages/tutorial",
    expectedIdentity: "dev.teamarness.tutorial",
    override: true,
    tier: "team",
    status: { kind: "admitted" },
  };

  it("lists refused Personal registrations with their diagnostic, Reload, and Remove", () => {
    const test = harness();
    const html = test.controller.renderRegistrations(snapshot([refused, admitted]));
    expect(html).toContain("com.example.broken");
    expect(html).toContain("/Users/person/Packages/broken");
    expect(html).toContain("A file changed after the manifest was written.");
    expect(html).toContain('data-package-registration-action="reload"');
    expect(html).toContain('data-package-registration-action="remove"');
    expect(html).toContain('aria-label="Reload com.example.broken"');
    expect(html).not.toContain("com.example.fine");
    expect(test.controller.renderRegistrations(snapshot([admitted]))).toBe("");
  });

  it("confirms Remove with copy that says the data is kept, then removes and re-reads", async () => {
    const test = harness();
    await test.controller.registrationAction("confirm-remove", refused.expectedIdentity);
    expect(test.names()).toEqual([]);

    await test.controller.registrationAction("remove", refused.expectedIdentity);
    const confirmation = test.controller.renderRegistrations(snapshot([refused]));
    expect(confirmation).toContain("Its data stays on this device");
    expect(confirmation).toContain('data-package-registration-action="confirm-remove"');
    expect(test.names()).toEqual([]);

    await test.controller.registrationAction("confirm-remove", refused.expectedIdentity);
    expect(test.calls).toEqual([["remove", refused.expectedIdentity]]);
    expect(test.refreshes()).toBe(1);
    // The row is gone from the next snapshot; its result stays visible.
    const after = test.controller.renderRegistrations(snapshot([]));
    expect(after).toContain("Removed. Its data stays on this device");
    expect(after).toContain('role="status"');
  });

  it("reloads a refused registration and reports a new refusal readably", async () => {
    const test = harness({ reload: async () => { throw { code: "artifact_missing" }; } });
    await test.controller.registrationAction("reload", refused.expectedIdentity);
    expect(test.refreshes()).toBe(1);
    const html = test.controller.renderRegistrations(snapshot([refused]));
    expect(html).toContain('role="alert"');
    expect(html).toContain("A file the manifest lists is missing. Rebuild the Package.");
  });

  it("offers Remove override, not Reload, for a Team Package override and then a restart", async () => {
    const test = harness({ remove: async (identity) => { test.calls.push(["remove", identity]); return { revision: null, restartRequired: true }; } });
    const html = test.controller.renderRegistrations(snapshot([override]));
    expect(html).toContain("Tutorial");
    expect(html).toContain("Override of a Team Package");
    expect(html).toContain("Remove override");
    expect(html).not.toContain('data-package-registration-action="reload"');

    await test.controller.registrationAction("remove", override.expectedIdentity);
    expect(test.controller.renderRegistrations(snapshot([override]))).toContain("Data is kept");
    await test.controller.registrationAction("confirm-remove", override.expectedIdentity);
    const after = test.controller.renderRegistrations(snapshot([override]));
    expect(after).toContain("Restart Teamarness to use the Team Package again.");
    expect(after).toContain('data-package-registration-action="restart"');
    await test.controller.registrationAction("restart", override.expectedIdentity);
    expect(test.names().at(-1)).toBe("restart");
  });

  it("shows a removed override as waiting for a restart and offers no second Remove", async () => {
    const removed: PackageRegistration = { ...override, status: { kind: "pending_restart", change: "removed" } };
    const test = harness();
    const html = test.controller.renderRegistrations(snapshot([removed]));
    expect(html).toContain("Applies after restart: removed from this device. The Team Package Tutorial returns when Teamarness restarts.");
    expect(html).toContain('data-package-registration-action="restart"');
    expect(html).not.toContain('data-package-registration-action="remove"');
    expect(html).not.toContain('data-package-registration-action="reload"');
    await test.controller.registrationAction("restart", removed.expectedIdentity);
    expect(test.names()).toEqual(["restart"]);
  });

  it("shows a written override as waiting for a restart, with Restart and Remove override", async () => {
    const written: PackageRegistration = { ...override, status: { kind: "pending_restart", change: "written" } };
    const test = harness();
    await test.controller.registrationAction("remove", written.expectedIdentity);
    const html = test.controller.renderRegistrations(snapshot([written]));
    expect(html).toContain("Applies after restart: this override replaces the Team Package Tutorial when Teamarness restarts.");
    expect(html).toContain("Remove the override?");
    test.controller.dismissFeedback();
    const idle = test.controller.renderRegistrations(snapshot([written]));
    expect(idle).toContain('data-package-registration-action="restart"');
    expect(idle).toContain("Remove override");
  });

  it("re-reads the snapshot after an override is written, so it shows as pending", async () => {
    const test = harness({ inspected: tutorialOverride });
    await test.controller.installAction("open");
    await test.controller.installAction("choose");
    await test.controller.installAction("continue");
    await test.controller.installAction("trust");
    const before = test.refreshes();
    await test.controller.installAction("override");
    expect(test.refreshes()).toBe(before + 1);
    expect(test.controller.state().kind).toBe("restart");
  });

  it("offers Remove, not Reload, for an entry written by hand for a Team Package identity", async () => {
    const handWritten: PackageRegistration = {
      directory: "/Users/person/Packages/tutorial",
      expectedIdentity: "dev.teamarness.tutorial",
      override: false,
      tier: "personal",
      status: { kind: "refused", code: "override_required" },
    };
    const test = harness({ remove: async (identity) => { test.calls.push(["remove", identity]); return { revision: null, restartRequired: true }; } });
    const html = test.controller.renderRegistrations(snapshot([handWritten]));
    expect(html).toContain("without an override. The Team Package stays unavailable until you remove it and restart Teamarness.");
    expect(html).not.toContain('data-package-registration-action="reload"');
    expect(html).toContain('aria-label="Remove Tutorial"');

    await test.controller.registrationAction("remove", handWritten.expectedIdentity);
    expect(test.controller.renderRegistrations(snapshot([handWritten])))
      .toContain("Remove this entry? The Team Package Tutorial returns after Teamarness restarts. Data is kept.");
    await test.controller.registrationAction("confirm-remove", handWritten.expectedIdentity);
    expect(test.calls).toEqual([["remove", handWritten.expectedIdentity]]);
    const after = test.controller.renderRegistrations(snapshot([{ ...handWritten, status: { kind: "pending_restart", change: "removed" } }]));
    expect(after).toContain("Removed from this device. Restart Teamarness to use the Team Package again.");
    expect(after.match(/data-package-registration-action="restart"/g)).toHaveLength(1);
    expect(after).not.toContain('data-package-registration-action="remove"');
  });
});
