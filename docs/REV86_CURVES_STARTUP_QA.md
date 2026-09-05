# Revision 86: relative premium upgrade curves and optional startup presentation

Revision 86 was built and statically checked but **not installed**. Additional
user feedback during packaging is included in revision 87, described below.

## User changes

- Level 1 displaying one chocolate never meant a constant-one price. Revision
  85 already used a positive float32 slope terminating at ten, with repeated
  integer prices at early levels. The new request also preserves expensive
  original items by allowing the final upgrade to cost up to twenty.
- Compare original final upgrade prices within Skill/Unit, area and currency.
  The lowest positive terminal price maps to cap 10, highest to 20, intermediate
  caps use linear interpolation rounded half up. A single distinct price maps
  to cap 10. Free items remain free. Original currency and unlock requirements
  are unchanged. This rule lives in both identical policy manifests.
- CH1 Skill caps are 10/15/20; CH2 Skill caps are also 10/15/20. Each starts
  at one, using the previously audited target-level offsets (Skill 2, Unit 1),
  client float32 arithmetic and integer truncation. Skill Reset remains free
  with its separate five-minute cooldown.
- Candy furniture uses each Prop's own PropLevel sequence, beginning at one
  and increasing with target level. Relative original terminal prices determine
  caps 10..20. This replaces the old whole-table rank mapping, which did not
  give every furniture item its own full progressive curve. All 17 furniture
  entries in the supported original master belong to CH1 and use Candy.
- The PropLevel client table, server master projection and variety-store
  transaction now use the same per-prop curve. Query prices reflect the stored
  level; purchase calculation rereads level inside the transaction. Explicit
  duplicate requests cannot deduct twice. Cosmetic purchase prices stay ten
  in their original currency, independent of these upgrade curves.

## Startup

- An installation-level, persisted Quick start switch is available before Start.
  Off uses 1.2x the previous cosmetic delays; On prints the ending immediately
  without typing, spinner or artificial delays. It never bypasses actual server,
  database, identity, SDK preparation or runtime-observer checks.
- The switch has all twelve supported locale variants and an English fallback.
  Start freezes the selected value. Diagnostic log polling remains at its real
  cadence, not slowed by the cosmetic multiplier.
- Initial launch no longer asks for document access. Only explicit Import/Export
  actions open the storage picker. Import still requires its overwrite warning.
- Lower controls scroll if necessary; the Start button stays outside that scroll.

## Verification / device boundary

- Added formula cap/grouping, float32 endpoint, full-level progression and atomic
  deduction/replay tests covering caps 10, 15 and 20. The read-only
  `tools/audit_formula_curves.py` transforms the user's verified original tables
  in memory and enumerates every Skill/Unit/Prop price without touching saves.
- Python 36 tests and the regular Rust workspace suite passed before packaging.
- Final package parity, installation and runtime observations are recorded below
  when completed. Source tests alone do not establish installed client behavior.
- User first requested in-place installation, then explicitly superseded it with
  uninstall/reinstall. Only the memorial package on the primary OnePlus may be
  removed; the original app and Samsung are out of scope. No phone reboot,
  repository commit or push is authorized.

## Revision 87: gingerbread send failure and original affection thresholds

- On the primary phone, screenshot `build/rev85-gift-error-current.png` shows
  `amount must be finite and non-negative (ERROR_100001)` in Joie's gift page.
- Bounded observation of the existing request dispatcher (RVA 0x1F08BD0) recorded
  `profile=1, gift=1, quantity=-5` from a normal single click. It did not alter
  arguments, return values, or inventory. The observer was detached afterward.
- Read-only observation of the current DB plus WAL showed Joie experience 120,
  level 2, with 39 gingerbread. This is valid under original thresholds (next
  level 150), but the client's previously scaled next threshold was 15.
  `ceil((15 - 120) / 20) = -5` explains the emitted negative count. The initial
  database backup was stale and was not used as the current-state conclusion.
- User explicitly chose **original affection thresholds for every profile,
  including Lily**, because gift/CH3 production is already accelerated. The
  client bundle and server master projection now preserve original thresholds;
  gifts, encore, ads and CH3 use the same unscaled cumulative thresholds. No
  negative-count coercion or inventory-validation bypass was added.
- Added private-fixture regression: six individual gingerbread sends produce
  experience 120 / level 2, then a three-item send produces 180 / level 3.
  Inventory decrements exactly, duplicate RPCs do not deduct twice, and negative
  requests leave inventory unchanged. All profile mutation thresholds are also
  compared to the actual packaged client table, not just the published DTO.
- Removed misleading `1, 10, 100` examples from currency descriptions in all
  twelve languages. Integer validation and multiplier-versus-count semantics
  remain unchanged.

## Final artifact QA and revision 88 packaging correction

- Revision 87 packaged master: all 508,626 rows in 28 ordinary upgrade tables
  retain the audited 0.1x cost transformation with other columns unchanged;
  all 440 FollowerProfileLevel rows, every column, match the original master.
- Re-ran all four private-fixture transport tests against the actual revision 87
  bundle: 54,371 projected scalar fields; 29,520 CH3 loadouts across 41 stages;
  every unlocked stage prefix after reopen; all 42 guide stages / 126 tasks,
  including replay and USN isolation. All passed. Gift transaction regression
  is included in the master parity test. Patch Python tests: 37 passed.
- The authorized uninstall of memorial revision 85 succeeded on the primary
  OnePlus, followed by installation of revision 87. Original game was untouched.
  First screen showed the localized Quick start switch and Import/Export buttons,
  with no automatic import permission popup. Screenshot: rev87-first-launch.png.
- Starting revision 87 failed before Unity with native error -13. Root cause:
  Rust and policy were rebuilt, but bootstrap.so retained its earlier compiled
  policy SHA-256. This was a packaging omission, not a corrupt save. Revision 87
  is NOT a runtime-accepted artifact despite its gameplay fixture tests passing.
- Rebuilt native Patch against the current server/policy. Added CMake policy-file
  configure dependency so edits trigger hash regeneration; native startup now
  logs both non-secret policy fingerprints on mismatch, without bypassing checks.
- Patcher now validates BOTH bootstrap and server embedded policy fingerprints
  before processing the package. Added a stale-bootstrap/fresh-server regression;
  all 14 patcher-core tests passed. Revision 88 carries this correction and the
  same gameplay/Java payload as 87. It will update the just-clean-installed app,
  without another data deletion or phone reboot.
- Revision 88 XAPK SHA-256:
  `776363402E098FA48564C01A4D7DBA306FA952C8F19C91824BB27134519F880F`.
  All three split signatures verify with the existing local test certificate.
- Revision 88 update installation succeeded; device reports versionCode 800088
  and versionName 8.0.0-memorial.88, PID 21339. Before the agent clicked Start,
  the user had already entered the game: `build/rev88-before-start.png` actually
  shows the in-game follower/profile page, including Lily at affection level 1.
  Passive process-scoped logs show the Unity client sending numbered userSave
  requests to its embedded loopback endpoint (port 42597). Startup -13 is no
  longer blocking. No further game taps or save mutations were made by the agent.
- Single-send/long-press gift behavior in the new phone save remains user runtime
  acceptance work; the private fixture regressions are not claimed as device
  coverage of those interactions. No additional uninstall after revision 87.
