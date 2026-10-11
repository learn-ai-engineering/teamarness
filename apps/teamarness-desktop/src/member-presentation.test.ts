import { describe, expect, it } from "vitest";
import type { Installation, Membership, PersonNameSnapshot } from "./contracts";
import { presentMembers } from "./member-presentation";

const local = "a".repeat(64);
const remote = "1".repeat(64);
const installation: Installation = { identity: local, displayName: "Device" };
const workspaceId = "00112233445566778899aabbccddeeff";
const membership: Membership = {
  workspaceId, epoch: "2", version: "b".repeat(64),
  members: [{ identity: remote, role: "member" }, { identity: local, role: "manager" }],
};
const person = (identity: string, name: string | null) => ({ identity, personId: "c".repeat(64), name, fingerprint: identity.slice(0, 12) });
const names = (persons: PersonNameSnapshot["persons"]): PersonNameSnapshot => ({ workspaceId, cursor: "1", persons });

describe("Workspace Person presentation", () => {
  it("keeps a stable fingerprint when names have not arrived and exposes full identities", async () => {
    const rows = await presentMembers(installation, membership);
    expect(rows[0]!.name).toBe(`Member ${remote.slice(0, 12)}`);
    expect(rows[0]!.detail).toContain(remote);
    expect(rows[1]!.name).not.toBe(installation.displayName);
  });

  it("shows Unicode and duplicate names as text with distinguishing identities", async () => {
    const rows = await presentMembers(installation, membership, names([person(remote, "👩‍💻"), person(local, "👩‍💻")]));
    expect(rows.map(({ name }) => name)).toEqual(["👩‍💻", "👩‍💻"]);
    expect(rows[0]!.detail).toContain("Duplicate name");
    expect(rows[0]!.detail).toContain(remote);
    expect(rows[1]!.detail).toContain(local);
  });

  it("does not inherit names from a different Workspace", async () => {
    const other = { ...names([person(remote, "Other")]), workspaceId: "f".repeat(32) };
    const before = (await presentMembers(installation, membership))[0]!.name;
    expect((await presentMembers(installation, membership, other))[0]!.name).toBe(before);
    expect((await presentMembers(installation, membership, names([person(remote, null)])))[0]!.name).toBe(before);
    expect(before).toBe(`Member ${remote.slice(0, 12)}`);
  });
});
