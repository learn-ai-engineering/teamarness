# 01-product — Product Requirements

Prefix: `TEAM.PROD`

## TEAM.PROD-R01 — Team Workspace

Teamarness SHALL serve as a functional team workspace application. A team with developers and non-developers SHALL be able to install the application, connect as a workspace, and use it for planning and communication without any server infrastructure beyond Iroh's default relays and a static file host for updates.

## TEAM.PROD-R02 — Success Criteria

The application SHALL satisfy the success criteria defined in `context/vision.md`:
1. A team installs, connects as a workspace, and uses it for planning and communication without server infrastructure.
2. A developer builds and ships a new package in under a day.
3. Two peers exchange encrypted messages within seconds.
4. A non-developer joins via invitation without a Git account.
5. A team outside the maintainers forks the app, adds a custom package, and uses it.

## TEAM.PROD-R03 — Audience

The application SHALL be usable by developers and non-developers from the same workspace. Role determines what the user sees, not the binary they run. Non-developer users SHALL NOT need a Git account, terminal, or IDE to participate in conversations and planning.