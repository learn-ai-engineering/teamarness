# Package source contracts v1

This directory defines the JSON documents consumed during Package Admission.

- `teamarness.package.json` is UTF-8 JSON limited to 256 KiB. It declares Package-owned identity, compatibility, artifacts, and UI contracts. It cannot declare source, trust, registration scope, override policy, or data paths.
- `application-packages.v1.json` is a build-generated list of normalized Package Directory paths relative to the application resource root.
- `<app-data>/packages/registrations.v1.json` is an owner-only installation file. Local directories are absolute. Trust requires the exact `trusted_native_code_v1` risk set. `overrideApplication` must be true to replace an application registration.
- UI contracts use JSON Schema draft 2020-12. Admission rejects non-local references, dynamic or recursive reference keywords, cycles, schemas deeper than 64 values, and schemas larger than 16,384 values before compilation. Schema retrieval is disabled in the host dependency.

Unknown fields fail each host-owned document. A manifest may carry Package-owned values only inside `extensions`, whose keys are reverse-DNS namespaces. JSON objects reject duplicate keys before schema decoding.
