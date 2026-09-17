# DELTA-005 — Protection Grants and stored secrets are weaker than "sealed" and "custody" suggest

**Status:** open (2026-09-18)

## Divergence

`TEAM.SYS.CONV-R11`, `TEAM.SYS.PKG-R03`, and ADR 0008 describe Core key custody and sealed records. The key that wraps a Protection Grant is derived from public values, so anyone holding the grant bytes can unwrap the Content Key. The Installation secret and Content Keys are stored as plaintext in the Core SQLite database.

## Impact

Confidentiality rests on grant bytes never leaving Core and on the data directory staying private.

## Resolution plan

The architecture-review workstream (see `roadmap.md` Current work), remediation plan phase 7 — key custody. Recipient-bound grant sealing lands earlier, in remediation plan phase 3. The remaining choice is an OS keychain or an accepted-risk decision record with corrected wording.

## Tracking

- Review findings SEC-02/SYN-02, SEC-08, SEC-10, SEC-11
- `TEAM.SYS.CONV-R11`, `TEAM.SYS.PKG-R03`, ADR 0008
