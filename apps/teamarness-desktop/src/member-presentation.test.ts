import { describe, expect, it } from "vitest";
import type { Installation, Membership, PrincipalsSnapshot } from "./contracts";
import { presentMembers } from "./member-presentation";

const local = "a".repeat(64);
const remote = "1".repeat(64);
const installation: Installation = { identity: local, displayName: "Device" };
const workspaceId = "00112233445566778899aabbccddeeff";
const membership: Membership = {
  workspaceId, epoch: "2", version: "b".repeat(64),
  members: [{ identity: remote, role: "member", principalId: "c".repeat(64), kind: "person", stewardPrincipalId: null }, { identity: local, role: "manager", principalId: "d".repeat(64), kind: "person", stewardPrincipalId: null }],
};
const person = (identity: string, name: string | null) => ({ principalId: identity === local ? "d".repeat(64) : "c".repeat(64), kind: "person" as const, stewardPrincipalId: null, keys: [identity], name, fingerprint: identity.slice(0, 12) });
const names = (principals: PrincipalsSnapshot["principals"]): PrincipalsSnapshot => ({ workspaceId, cursor: "1", principals });

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

describe("Workspace Agent presentation", () => {
  const agentIdentity = "e".repeat(64);
  const agentId = "f".repeat(64);
  const agent = { identity: agentIdentity, role: "member" as const, principalId: agentId, kind: "agent" as const, stewardPrincipalId: "c".repeat(64) };
  const agentRow = { principalId: agentId, kind: "agent" as const, stewardPrincipalId: "c".repeat(64), keys: [agentIdentity], name: "Helper <script>", fingerprint: agentIdentity.slice(0, 12) };
  const roster = { ...membership, members: [...membership.members, agent] };
  const profiles = names([person(remote, "Remote steward"), person(local, "Manager"), agentRow]);

  it("shows kind, steward, full identity only in details, and Manager removal without rename", async () => {
    const rows = await presentMembers(installation, roster, profiles);
    expect(rows[2]).toMatchObject({ name: "Helper <script>", indicator: "Agent · stewarded by Remote steward", role: "Member", canRemoveAgent: true, canRenameAgent: false, agentId });
    expect(rows[2]!.detail).toContain(agentIdentity);
    expect(rows[0]!.canRemoveAgent).toBe(false);
  });

  it("allows the steward to rename and remove, but not unrelated members", async () => {
    const steward = { identity: remote, displayName: "Steward" };
    const stewardRoster = { ...roster, members: roster.members.map((member) => member.identity === remote ? { ...member, role: "member" as const } : { ...member, role: "member" as const }) };
    expect((await presentMembers(steward, stewardRoster, profiles))[2]).toMatchObject({ canRemoveAgent: true, canRenameAgent: true });
    const unrelated = { ...stewardRoster, members: [...stewardRoster.members, { identity: "9".repeat(64), role: "member" as const, principalId: "8".repeat(64), kind: "person" as const, stewardPrincipalId: null }] };
    expect((await presentMembers({ identity: "9".repeat(64), displayName: "Member" }, unrelated, profiles))[2]).toMatchObject({ canRemoveAgent: false, canRenameAgent: false });
  });
});
