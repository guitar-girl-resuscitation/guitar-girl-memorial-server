# Activity, CH3 and offline regression checkpoint

This is a source/test checkpoint, not a device acceptance or full parity claim.
The reference remains production captures, then tested `reborn-server` Rust and
`reborn-hook` LSPosed behavior. Do not replace these with another server project.

## Fixed in source

- Guide rewards (separate from Samseck): the verified master has 42 active
  Type=1 stages, each with three independent tasks, and 26 active Type=2 rows.
  The rewrite previously chose the next greater active ID, erroneously moving
  42 to 902. The tested Rust increments the ID to terminal sentinel 43 instead.
  `setFollowerQuestInfinite` preserves that sentinel and its completed ID 42.
- Guide `I_CompleteID` now comes from this USN's completed-stage ledger. Login
  previously returned zero whenever the current stage was incomplete; task RPCs
  incorrectly returned the current ID even before all tasks completed. Neither
  projection now guesses completion from the currently displayed task. Exact
  request replays also clear Next_flg as well as the reward payload.
- Guide testing exposed a shared reward defect: type 6 / ID 2 Fever was treated
  as unique ownership, so later independent rewards disappeared. It now extends
  `max(device_now, persisted_deadline) + reward_seconds` transactionally; request
  retries extend nothing. Full reload emits User_buff with both absolute times.
  This adds USN-scoped `buff_timers` to development schema 4. No existing device
  database was opened, migrated, erased or modified; old development schemas
  remain explicitly unsupported, per the clean-rebuild requirement.
- Full login/load omitted User_follower_profile, User_follower_giftitem and
  User_follower_profile_reward despite durable data. All three are now projected
  from their USN-scoped records. Preserve the old fresh-profile boundary and the
  built-in Joie fallback; do not expose every locked profile from master data.
- Samseck's database completion marker existed, but full player projection
  omitted `U_samseck_step`. `PlayerSnapshot` now reads the durable marker and
  login/user-load projection emits it. A zero-only fresh-save comparison had
  missed this regression. The wire oracle now checks completed steps 1, 2, 3
  against the tested Rust serializer, not just field/container shapes.
- CH3 `UserApData.I_FullApTime` was incorrectly sent as a remaining duration.
  It now sends the absolute Unix deadline, using the persisted calculation
  timestamp and one-point-per-second recovery. At full capacity it preserves
  the old Rust `now+1` sentinel. No timezone offset belongs in this timestamp.
- CH3 score projection preserves request order while deduplicating profiles,
  matching the old Rust implementation. A missing Music_id uses the selected
  CH1 music, also matching that baseline. Scoring consumes a read-only catalog,
  so the numerical oracle needs no HTTP process or mutable player database.

## Regression checks

- Samseck: claim, same-request replay, mail claim, relogin, close/reopen database,
  new-request retry and a different USN. The completion survives; mail and
  chocolate are not granted again. Other-slot state remains empty.
- Guide chain: four consecutive stages, closing/reopening the database between
  every task. All three condition values/claim flags remain independent;
  next-stage flags start empty; both transport replay and new-request duplicate
  claims issue no additional reward. Twelve tasks grant exactly twelve fixture
  rewards, only to their USN. This is persistence coverage, not visual UI proof.
- Full private-master guide regression additionally executes all 42 stages /
  126 tasks through the real handler, including below-threshold updates,
  out-of-display-order claims, exact replay, new-request duplicates, and an
  actual writer-thread shutdown / database reopen after each task. CP/candy
  settlement, Fever extension, previous-stage records, login condition/claim
  fields, terminal 43/infinite state and another USN are checked. Saved response
  scalars use the old Rust serializer as oracle. Its private transient Next_flg
  and Reward_data fields use separately derived expected outcomes/master rewards,
  not an invocation of the old private command handler. Type=2 recurrence is
  not claimed covered by this main-chain test.
- CH3 entry: compare the complete getChThird response before any completion
  and after each of all 41 nodes, closing/reopening between entries. Apart from
  the explicit memorial AP-cap policy and the oracle's wall-clock timestamp,
  all fields match the tested Rust. This does not establish Unity scene health.
- Profile/gift/claim reload: ordinary follower + Lily, nonconsecutive claimed
  levels 1 and 3, gift inventory, duplicates, database reopen and another USN;
  all three login lists match the tested Rust wire. Expired and active Fever
  full-login wire trees are also compared against that serializer.
- CH3: a private master supplied from the pinned 8.0.0 package was used to
  compare 29,520 loadouts across all 41 nodes with the old Rust scoring code.
  Includes Lily/music/follower levels 1, 20/21, 50/51 boundaries, empty and
  duplicate/reordered follower selections, recommended/nonrecommended music,
  all score components and FAIL/one/two/three-star results. All matched.
- Energy: close/reopen without a running worker, elapsed seconds, repeated
  same-time query, backward clock followed by forward time, cap and USN isolation.
  Absolute AP deadline wire tests pass separately from recovery tests.
- Consume fans: all owned costume bonuses remain in the calculation. Mail,
  Shop and Pass share persistent settlement; both areas and replay were tested.
  Production-multiplier Likes/notes are still unfinished; see the Consume doc.

Private scoring test:

```text
GGFM_ORIGINAL_MASTER=<private original.sqlite>
cargo test -p ggfm-transport-http all_ch3_stage_scores -- --ignored --nocapture
cargo test -p ggfm-transport-http complete_master_guide_chain -- --ignored --nocapture
cargo test -p ggfm-transport-http ch3_entry_every -- --ignored --nocapture
```

## Offline Likes: evidence, not yet repaired

The stock client owns this flow in `HibernationManagerSGT`, not a server ticking
task. `InitHibernation` at 0x2100A30 loads two durable PlayerPrefs values through
DataManager (0x205F98C / 0x205FB4C): `Key_HibernationStartTimeTicks` (enum 165)
and `Key_HibernationTotalTimeTicks` (166). It then runs eligibility checks and
the reward calculation before refreshing the start timestamp. Do not replace
this by granting based on every login or by accumulating server uptime.

`CalcHibernationTotalTime` at 0x2101244 subtracts stored start ticks from the
client clock, checks its configured minimum, adds pending elapsed time, and
caps at the original limit. `GetNow` (0x1CF18A0) uses DataManager's synchronized
clock. The baseline has no separate server-driven offline-award implementation.
The actual missing-reward cause is still unproven: inspect persisted ticks,
eligibility, production cache and clock initialization on a fresh device run.
Do not disable stock checks or add speculative rewards merely to show a popup.

The PlayerPrefs GetString single-argument overload (0x3ADA77C) delegates to the
already hooked two-argument overload. Static inspection did not prove missing
overload interception or double namespacing. Preserve USN isolation.

## Device/build boundary and next steps

- User authorized a temporary Samsung USB test after static tests. Read-only
  identification found RFCT408KQ7M / Samsung SM_A336E. Memorial package was not
  installed there, so it cannot supply the originally reported crash trace.
- CH3 entry crash is still OPEN. Score-oracle/AP fixes do not establish that a
  Unity scene can be entered. Need a current build and real crash trace or a
  successful repeated entry/settlement/relaunch test before closing it.
- Patch parity: restored the old LSPosed BuyAP-click guard at verified v8 RVA
  0x22A3F6C (void OnUIEventClickBuyAP, ELF file offset 0x229FF6C). Its 16-byte
  prologue is pinned in the compatibility manifest. This only prevents opening
  the retired AP shop popup; it must not be described as an entry-crash fix.
- Android Rust and Native have now been rebuilt and linked together; the stale
  `ggfm_server_transfer_save` link failure is resolved. The internal diagnostic
  package is `build/ggfm-e2e-smoke-82.xapk`, SHA-256
  `68B23F9C899886AEA58B44F8B647FB2289D92B7A51B4D544EEE2F9A6126B0773`.
  It uses `build/bootstrap-java-rev81-currency-input/classes.dex`. Currency
  multiplier/admin limitations are still open; this is not a release candidate.
- Private QA inputs extracted from that exact output package are under
  `build/master-static-rev82`. All 54,371 static fields / 38 tables agree with
  the server policy; all four private tests passed (master, guide, score, entry).
- Rev82 installed on Samsung RFCT408KQ7M only, where memorial was absent.
  Bootstrap activity survives. Runtime/CH3 testing is in progress; do not call
  this a successful CH3 acceptance. No save wipe, phone reboot, commit or push.

### Samsung runtime checkpoint / user takeover

- Rev82 reached the original introduction, localized memorial notice and CH1
  home on a fresh Samsung slot. Skipping the stock tutorial was saved across
  a process cold start. A guest/Play Games reminder still appears on relaunch.
- CH3 was initially correctly locked behind Maya Lv.1. For this disposable
  test slot only, used the memorial currency UI to send a diagnostic large
  Likes grant, then unlocked Maya and Joie through the stock follower UI.
  Production and fans visibly started increasing after unlocking Maya.
- Delivery showed success but the stock mailbox stayed empty until application
  cold start. The mail then appeared and could be claimed. Thus persistence
  worked for this grant, but immediate mailbox refresh is a confirmed open
  Patch defect. Currency list/input also fell back to English while the root
  menu and confirmation remained Chinese; do not mark localization accepted.
- Evidence: private `build/rev82-samsung-runtime.log`,
  `build/rev82-samsung-reopen-runtime.log`, `build/rev82-mail-to-claim.png`,
  `build/rev82-maya-unlocked.png`. Diagnostic grant does not validate production
  multiplier settlement or the normal progression curve.
- Stopped Samsung interaction when the user took over testing. CH3 scene entry
  itself has NOT yet been exercised; no claim that the reported crash is fixed.
- User device reconnected at `10.0.1.24:36687`; read-only package inspection
  reports rev80 / versionCode 800080. Rev82 uses schema 4 and does not migrate
  earlier test databases. Do not install then silently clear/recover the old
  test save; resolve a fresh-test installation choice with the user first.
- Latest normal `cargo test --workspace -j2` passed, including 39 transport and
  36 persistence tests; four private-master tests were run separately as above.
- User then explicitly requested uninstall/reinstall. Uninstalled only
  `org.guitargirlresuscitation.memorial` on `10.0.1.24:36687` (ADB Success),
  removing its old test data as requested. Rev82 installed successfully and
  bootstrap survived (PID 23838); user performs the gameplay test. This checks
  installation/bootstrap only, not CH3 acceptance. Original
  `com.neowiz.game.guitargirl` and Samsung data are untouched.

### Rev82 CH3 scene-entry crash: truncated mailbox GC handle

- User reproduced entry crash on the fresh rev82 installation. Device crash
  buffer at 2026-09-05 20:32:04 +1000, PID 5604, UnityMain: SIGSEGV at
  `libil2cpp.so+0x1a052b4`, invalid address `0x201c0020`. Caller is
  `MailboxStartHook+48` in the new native Patch, not a stage-score handler.
- The deployed bootstrap located the failure at `handle_free(mailbox_handle)`.
  Its old 32-bit declaration discarded the upper address bits of this handle.
  Verified create/store/get-target/free types must use `uintptr_t` on this
  supported ARM64 runtime. No assumption from a different Unity ABI is valid.
- Added a shared GC-handle ABI header and native regression with a synthetic
  high-bit handle across creation, storage, target lookup and release. Added
  full-width handle lifecycle logs to mailbox Start for scene-transition QA.
- The tested old LSPosed native Hook has the distinct retired CH3 BuyAP popup
  guard, but no mailbox GC-handle Hook. This crash was introduced by the new
  Patch, not evidence that the old map-null fix should be blindly repeated.
- Source fix is not scene-entry acceptance: deploy and repeat CH1/CH3 scene
  transitions, then check for complete handle release/recreation and absence
  of a new crash before marking runtime success. Existing test save preserved.
- Rev83 built and installed over rev82 on both devices without clearing data.
  User-device runtime confirms getChThird at 20:46:27, full handle
  `0x6f301c2031` release and recreation, followed by the actual CH3 1-1 preview
  screenshot. Evidence: `build/rev83-user-ch3-runtime.log` and
  `build/rev83-user-drop-preview.png`. The reported mailbox-free crash no
  longer blocks that transition. This does not certify every CH3 interaction.
- Rev83 private-master QA compared 54,371 fields / 38 tables, 29,520 CH3
  scoring loadouts / 41 nodes and 126 guide tasks; all passed. The independent
  native handle regression also ran successfully on Samsung ARM64.

### Follow-up: gift/shop artwork and memorial localization/input

- The packaged original FollowerGiftItem table assigns all six IDs to
  `icon_gift_gingerman`, despite distinct localized names. The user-owned
  `icon/icon_gift.ab` contains all corresponding textures: gingerman,
  chocomilk, sandwich, bread, grapefruit and coffee (including small variants).
  Shared policy now supplies the six correct references; Patcher changes only
  the resource-name fields after an exact ID/original-value check. Server
  master projection uses the same mapping. No game artwork is copied into
  source control, and gift IDs/quantities/affection values are not rewritten.
- Eight appended memorial shop rows incorrectly used icon_shop_01. They now
  use their matching stock Shop artwork: 09/10/11/12 for the four chocolate
  packs, 06/07 for beginner/master, 30 for premium and 05 for continuous tap.
- CH3 preview listing is generated locally: 0x209CBCC calls Percent's helper
  0x1FFFD10, which deduplicates by reward type + ID and constructs each example
  with quantity 1 (`fmov d0,#1` at 0x1FFFEF8). That original sample quantity is
  NOT the actual drop count or a guaranteed award. The 1-1 groups contain gift
  ID 1 by star, and gift ID 2 with 50% nonzero/50% zero quantity. Repairing the
  shared gift resource mapping corrects preview, mailbox and inventory artwork
  without changing drop identity or converting rare gifts into gingerbread.
- Currency UI received `zh-Hans` from the native menu but only recognized
  `zh_chs`, causing English fallback. Added canonical Chinese aliases and
  regional language normalization. Mail copy uses the shared locale normalizer;
  native menu error dialogs use localized copy instead of raw English bodies.
- Latest user direction supersedes decimal/compact input: all six visible
  currencies accept plain positive integers only. Likes/notes/fans interpret
  them as multipliers, candy/chocolate as counts; no decimals, A/K suffixes,
  exponent or addition syntax. Numeric keyboard and 12-language examples match.
- These follow-up changes are source work pending rev84 packaging/runtime QA;
  rev83 on the user's phone does not contain them. Furniture/skill price curves
  remain a separate pending change (desired smooth per-level progression to 10).
