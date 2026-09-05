# Revision 84: packaged curve audit and startup terminal

## Evidence and implementation

- Original verified 8.0.0 `FollowerCost` TextAsset contains 24 SQLite
  `Follower_*` tables. The earlier transform matched the asset against the
  table prefix, so rev83 left all these actual prices untouched. `FollowerCost`
  is now explicitly transformed and mandatory in the build assertion.
- `CharacterCost` contains CH1/CH2 CharacterCost1/2; `MusicCost` contains
  MusicCost_1/2 shared by the songs. `verify_packaged_curves.py` streams every
  row across all 28 tables in the original and final-package masters, checking
  cost x0.1 (original minimum/rounding policy), identical exponent digits,
  identical production and all remaining columns. No copyrighted fixture is
  stored in the public source trees.
- User examples: Follower_6 level 1 originally 9.68832e16, expected 9.68832e15;
  Follower_7 level 0 originally 5.66362e20, expected 5.66362e19.
- Skill client price helper RVA 0x21F0F94 computes float32
  `base + max(targetLevel - 2, 0) * increment`. Its UI at 0x1F8E58C calls
  0x1BAFE34 with current level +1, then truncates the result to int32.
- Unit/furniture helper RVA 0x221CC00 instead subtracts **1**, not 2
  (0x221CCB8); its UI at 0x2132788 calls 0x1D667D4 and truncates to int32.
  These observations come from the verified user-owned ARM64 ELF, not a
  guessed shared formula. No new native cost hooks or binary edits are needed.
- Positive premium base becomes 1; increment is 9/(maxLevel-offset), where
  offset is 2 for Skill and 1 for Unit. Free rows remain free. At the final
  paid upgrade the displayed and charged price is 10. Skill unlocking
  requirements, effects, and acquisition currency remain unchanged.
- Server master projection and offline bundle consume the same policy. Atomic
  purchase reads current level inside the transaction, applies the same
  float32 operations and integer conversion, then deducts and raises the level.
  Tests cover both offsets, every level, final price and idempotent replay.
- Other rank-mapped premium tables now have upper bound 10 instead of 9.

## Startup UI

- Terminal height increased from 42%/330dp to 53%/410dp, leaving transfer
  controls and localized Start visible below it. Existing safe-area insets
  and scrollable log region retained.
- Command text streams two characters per 18ms, followed by a 360ms ASCII
  spinner, detailed scene output, a blank line and an 1100ms inter-command
  pause. Final 16-cell progress line replaces the three-dot separator.
- Live log pumping stops during the final cosmetic sequence to prevent an
  incoming diagnostic line being overwritten by an in-place animation.
  Diagnostics are drained afterward and remain available in logcat.
- AIRISUTEK transport/memory milestone lines are emitted only after native
  startup succeeds. Runtime observer arming is still distinguished from
  gameplay hooks, which cannot install until Unity loads IL2CPP.
- Final command remains `> come back ...`; final line remains
  `Welcome home, Lily!`, held for at least one full second before Unity.
- Transfer labels/warnings and Start support all 12 languages; traditional
  Chinese script-only locales now also work. Real diagnostics remain English.

## Acceptance boundary

Revision 84 also carries the pending rev83 follow-ups documented in
`ACTIVITY_CH3_OFFLINE_REGRESSIONS_20260905.md`: gift/shop artwork, integer-only
currency UI, Chinese locale normalization, AP 200, original cosmetic currency
and GP price x0.1, pass selection list, and localized startup feature reminder.
Build/static checks and actual device checks are recorded below when completed.
Initial plan was an in-place update. The user subsequently explicitly requested
a clean reinstall on the primary OnePlus (not Samsung). Revision 83 of
`org.guitargirlresuscitation.memorial` was uninstalled on `10.0.1.24:36687`,
its absence verified, and revision 85 installed. This deletes the memorial
app's saves/cache. Original `com.neowiz.game.guitargirl` was not modified.
No phone reboot, commit or push occurred.

### Completed static/package checks

- Rust workspace: all regular tests pass, including atomic level-price/replay
  regression. Patch Python: 35 tests pass. Java terminal/currency/transfer
  localization checks pass. ARM64 Rust and native bootstrap builds succeed.
- Both final rev84 and rev85 packages: **508,626 rows in 28 cost tables** pass
  the original-to-patched full-row check. Patched table bundle SHA-256 is
  `822C71684FB80E61557BF3F2895034584E7A619C85293D4FF375D066D02124D2` in both.
- Final rev85 private-fixture tests: 54,371 fields across 38 server/client
  master tables; 29,520 CH3 loadouts across 41 stages; full guide chain
  42 stages / 126 tasks with replay, reopen and USN isolation all pass.
- Rev84 Samsung launch reached the home screen. An overlong notice clipped
  against the stock date label; rev85 shortens all 12 languages. Samsung's
  warm-cache skill display still showed an old cost; this is not accepted as
  proof of updated client pricing. Primary-phone clean-install observation
  is required and no claim of comprehensive device acceptance is made.
- Rev85 final XAPK SHA-256:
  `69DF5FBDDB59E46BB71C92F733B550DC046834EDCAC09579F3C5C4E9FB07F65D`.
- Primary install/startup PID 31495. Evidence log `build/rev85-user-runtime.log`.
