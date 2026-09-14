# Package source contracts v1

This directory defines the JSON documents consumed during Package Admission.

- `teamarness.package.json` is UTF-8 JSON limited to 256 KiB. It declares Package-owned identity, compatibility, artifacts, and UI contracts. It cannot declare source, trust, registration scope, override policy, or data paths.
- `application-packages.v1.json` is a build-generated list of normalized Package Directory paths relative to the application resource root.
- `<app-data>/packages/registrations.v1.json` is an owner-only installation file. Local directories are absolute. Trust requires the exact `trusted_native_code_v1` risk set. `overrideApplication` must be true to replace an application registration.

A UI contract is a closed version 1 document with `$defs`, `requests`, and `subscriptions`. A request operation declares separate `request` and `response` schemas. A subscription operation declares separate `request` and `event` schemas. Operation names are 1 to 128 ASCII bytes and match `^[a-z][a-z0-9_-]*(\.[a-z][a-z0-9_-]*)*$`.

Admission permits at most 128 request operations, 128 subscription operations, and 256 operations in total. It accepts only `#/$defs/...` references and rejects dynamic references, missing definitions, cycles, schema depth above 64, more than 16,384 schema nodes, and contract files above 1 MiB. The host compiles all four validator directions during admission. Generic request, response, subscription, and event payloads are limited to 256 KiB of encoded JSON and a depth of 64.

The UI entrypoint must be UTF-8 ESM and no larger than 8 MiB. A Package may declare at most 64 UI assets, each no larger than 8 MiB. The entrypoint, assets, and contract may total at most 32 MiB. The host copies admitted resources to its private cache and resolves them only through an opaque mount token. Source paths, cache paths, and admission digests are not public catalog values.

Unknown fields fail each host-owned document. A manifest may carry Package-owned values only inside `extensions`, whose keys are reverse-DNS namespaces. JSON objects reject duplicate keys before schema decoding.
