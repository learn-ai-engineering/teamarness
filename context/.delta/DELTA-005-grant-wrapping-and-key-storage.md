# DELTA-005 — Protection Grants and stored secrets are weaker than "sealed" and "custody" suggest

**Status:** closed (2026-09-19)

## Divergence

`TEAM.SYS.CONV-R11`, `TEAM.SYS.PKG-R03`, and ADR 0008 described Core key custody and sealed records. The key that wrapped a Protection Grant was derived from public values, so anyone holding the grant bytes could unwrap the Content Key. The Installation secret and Content Keys are stored as plaintext in the Core SQLite database.

## Impact

Confidentiality rested on grant bytes never leaving Core and on the data directory staying private.

## Resolution

Grants are sealed to their recipient by ADR 0013, implemented in remediation plan phase 3 Step 3b (commit `292ded1`): HPKE to an X25519 key derived from the recipient's Installation identity, with the whole header as associated data and a domain-separated signature.

Plaintext at rest is an accepted risk under ADR 0017, which states the real guarantee in ID-R01, ID-R04, the new ID-R07, PKG-R11, TRANS-R03, and the README, and records the triggers for revisiting platform key custody. The hygiene that ADR 0017 requires landed in commits `ee69476` (private data-directory rule, fail closed off Unix) and `1801461` (one stored copy of each Content Key, `secure_delete`, zeroizing key buffers).

## Tracking

- Review findings SEC-02/SYN-02, SEC-08, SEC-10, SEC-11
- `TEAM.SYS.CONV-R11`, `TEAM.SYS.PKG-R03`, `TEAM.SYS.ID-R07`, ADR 0008, ADR 0013, ADR 0017
