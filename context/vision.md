# Teamarness — Vision

## What This Is

Teamarness is a local-first, P2P team application built with Rust, Tauri, and Iroh. Teams share context, planning, conversations, and tooling without a central server. The Core Runtime owns Workspace identity, Canonical Membership, and generic synchronization machinery. Every product domain, including conversations, documents, repositories, and AI tooling, is a Package that owns its model, records, schemas, projections, policy, and UI.

## The Problem

Teams adopting AI tooling face a structural gap. Individual developers experiment freely with AI — prompting, agent workflows, custom skills — but there is no agreed model for how a team defines shared AI behavior, how individual members layer personal configuration on team agreements, or how non-developers participate in an AI-assisted workflow. Most teams have not figured out how to do both.

The standard tools compound the problem: SaaS-based, role-siloed, server-dependent. A developer's AI workflow lives in their IDE; a PM's lives in a project tracker; a designer's lives in Figma. There is no cohesive application where the full team shares the same context, each from their own vantage point.

## The Values

- **Local-first, no required server.** The app works offline. Peers synchronize directly over Iroh's QUIC transport. No central server mediates content or authentication. Iroh relays handle NAT traversal edge cases but carry no content authority — all traffic is end-to-end encrypted.
- **Extensible through Packages.** All product-domain behavior lives in Packages. The Core Runtime provides identity, membership, signing, and generic synchronization through versioned Semantic Capabilities. The Shell provides host-level Workspace controls and Package UI mounting. The Package Process Protocol does not require a Backend language, though available SDKs determine which languages have supported tooling.
- **AI as a package concern.** AI tooling — agents, skills, prompts, workflows — is owned by packages, not the runtime. Team-agreed AI behavior is checked in and shared. Individuals layer personal configuration on top.
- **Git remains the source of truth for code.** Teamarness reads repositories; it does not replace version control.
- **A working demonstration of P2P for enterprise.** This is software teams can point to as evidence that local-first and P2P are viable for real team tooling.
- **The whole team in one context.** Developers, PMs, designers, testers, and leadership operate from the same shared picture of the project.

## What Teamarness Is Not

- Not a Git hosting service or GitHub replacement.
- Not a SaaS product. No hosted version, no subscription.
- Not a general-purpose chat tool replacing Slack or email.
- Not a task tracker. External trackers are surfaced by packages; Teamarness does not replace them.
- Not a centralized analytics platform.
- Not a security boundary between team members. The trust model assumes colleagues on the same team.

## Success Criteria

1. A software team with developers and non-developers installs Teamarness, connects as a workspace, and uses it for planning and communication without any server infrastructure beyond Iroh's default relays and a static file host for updates.
2. A developer registers their repository and sees Teamarness package views (documentation, architecture, backlog) inside the workspace app without running a separate CLI or web server.
3. A Package author builds and ships a new Package to their team in under a day using a supported Backend and UI SDK.
4. Two team members on separate networks, behind NAT, can collaboratively edit a planning document with changes visible within two seconds.
5. A non-developer team member joins the workspace via an invite token and participates in conversations and planning without a Git account.
6. An update published by a CI pipeline reaches all team members as a notification within one hour; installation requires a single click.
7. At least one team outside the Teamarness maintainers forks the app, adds a custom package, and uses it in their own workflow.