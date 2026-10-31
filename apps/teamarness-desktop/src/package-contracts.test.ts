import { expect, it } from "vitest";
import fixture from "../contracts/fixtures/package-bridge.json";
import type {
  InspectedPackage,
  OpenPackageMount,
  PackageAuthoringEntry,
  PackageLoader,
  PackageRegistration,
  PackageUiView,
} from "./package-contracts";

// Each key set lists every field of its TS type, so `satisfies` fails to compile when the
// type gains or loses a field, and the runtime check fails when the fixture drifts.
const registrationKeys = {
  directory: true, expectedIdentity: true, override: true, tier: true, status: true,
} satisfies Record<keyof PackageRegistration, true>;
const inspectedKeys = {
  directory: true, identity: true, version: true, displayName: true, description: true, author: true,
  homepage: true, capabilities: true, provided: true, consumed: true, kind: true,
} satisfies Record<keyof InspectedPackage, true>;
const authoringKeys = {
  identity: true, capabilities: true, provided: true, consumed: true,
} satisfies Record<keyof PackageAuthoringEntry, true>;
const consumedAvailabilityKeys = {
  identity: true, major: true, minMinor: true, maxMinor: true, required: true, provider: true, minor: true,
  unavailable: true,
} satisfies Record<keyof PackageAuthoringEntry["consumed"][number], true>;
const loaderKeys = {
  token: true, entrypoint: true, uiHostVersion: true, contractDigest: true, package: true, workspace: true,
  view: true, nestedContent: true,
} satisfies Record<keyof PackageLoader, true>;

const tiers = { team: true, personal: true } satisfies Record<PackageRegistration["tier"], true>;
const pendingChanges = { written: true, removed: true } satisfies Record<
  Extract<PackageRegistration["status"], { kind: "pending_restart" }>["change"],
  true
>;
const inspectedKinds = { new: true, reload: true, replaces_registration: true, overrides_team: true } satisfies Record<
  InspectedPackage["kind"],
  true
>;
const views = { main: true, settings: true } satisfies Record<PackageUiView, true>;

function expectKeys(value: object, keys: object) {
  expect(Object.keys(value).every((key) => key in keys)).toBe(true);
}

it("keeps wide event sequences as decimal strings and bridge fixtures path-free", () => {
  expect(fixture.event.sequence).toBe("9007199254740993");
  const encoded = JSON.stringify(fixture);
  for (const forbidden of ["sourcePath", "cachePath", "dataPath", "processHandle", "secret", "transportHandle"]) {
    expect(encoded).not.toContain(forbidden);
  }
});

it("reports whether a mount may nest content frames", () => {
  expect(fixture.loader.nestedContent).toBe(false);
  expect(fixture.openMount.loader.nestedContent).toBe(true);
});

it("covers both Package UI views with the loader shape", () => {
  const loaders = [fixture.loader, fixture.openMount.loader];
  for (const loader of loaders) {
    expectKeys(loader, loaderKeys);
    expect(Object.keys(loader)).toHaveLength(Object.keys(loaderKeys).length);
    expect(loader.view in views).toBe(true);
  }
  expect(loaders.map((loader) => loader.view).sort()).toEqual(Object.keys(views).sort());
  const open = fixture.openMount as OpenPackageMount;
  expect(open.mount).toBe("mount-2");
  expect(open.loader.package.lifecycle).toBe("active");
});

it("accepts every registration status the Rust DTO writes", () => {
  const registrations = fixture.snapshot.registrations as readonly PackageRegistration[];
  for (const registration of registrations) {
    expectKeys(registration, registrationKeys);
    expect(Object.keys(registration)).toHaveLength(Object.keys(registrationKeys).length);
    expect(registration.tier in tiers).toBe(true);
    if (registration.status.kind === "pending_restart") {
      expect(registration.status.change in pendingChanges).toBe(true);
    }
  }
  expect(registrations.map((registration) => registration.status)).toEqual([
    { kind: "admitted" },
    { kind: "refused", code: "port_provider_conflict" },
    { kind: "pending_restart", change: "written" },
    { kind: "pending_restart", change: "removed" },
  ]);
  expect(registrations.map((registration) => [registration.tier, registration.override])).toEqual([
    ["personal", false],
    ["personal", false],
    ["team", true],
    ["team", true],
  ]);
});

it("reads the inspected Package preview the Rust DTO writes", () => {
  const inspected = fixture.inspectedPackage as InspectedPackage;
  expectKeys(inspected, inspectedKeys);
  expect(inspected.kind in inspectedKinds).toBe(true);
  expect(inspected.displayName).toBe("Notes");
  expect(inspected.capabilities[0]).toEqual({
    identity: "teamarness.workspace-context", major: 1, minMinor: 0, maxMinor: 0,
  });
  expect(inspected.provided[0]?.minor).toBe(2);
  expect(inspected.consumed[0]?.required).toBe(false);
});

it("reads routed and unavailable consumed Ports from the authoring catalog", () => {
  const catalog = fixture.authoringCatalog as readonly PackageAuthoringEntry[];
  const [entry] = catalog;
  expect(entry).toBeDefined();
  expectKeys(entry!, authoringKeys);
  for (const consumed of entry!.consumed) {
    expectKeys(consumed, consumedAvailabilityKeys);
    expect(Object.keys(consumed)).toHaveLength(Object.keys(consumedAvailabilityKeys).length);
  }
  const [routed, unavailable] = entry!.consumed;
  expect(routed).toMatchObject({ provider: "dev.teamarness.exhibits", minor: 0, unavailable: null });
  expect(unavailable).toMatchObject({ provider: null, minor: null, unavailable: "port_unavailable" });
});
