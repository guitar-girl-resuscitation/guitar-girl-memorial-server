# Tested Rust / LSPosed baseline repair checkpoint

This is a repair checkpoint, not a declaration of gameplay parity. Production
traffic remains the first source of wire evidence; `reborn-server` and
`reborn-hook` remain the tested gameplay and client-compatibility baselines.
Third-party server implementations are not acceptance references.

## Server and table fixes

- Selection/equipment IDs in an area snapshot no longer grant ownership.
  New content requires an explicitly positive level or a business acquisition.
  Locked/ID-only music snapshots cannot create orphan encore timers.
- Fresh content reward levels and absent encore follower IDs use the observed
  zero sentinels. Encore response/request field numbers remain distinct.
- Premium upgrade prices rank original prices numerically within each system
  and currency. Row-specific writes prevent a replacement value from being
  treated as another original price later in the same transformation.
- Heart/like/note upgrade curves scale their cost mantissa by 0.1, retain
  magnitude exponents, and do not modify production amounts. Only Candy/CP
  costs use the single-digit policy.
- Cosmetic numeric bonuses and every localized percentage description agree.
  Default equipment and content acquisition provenance remain unchanged.
- Level-column matching is case-insensitive, including `i_unlockLevel`.
- The embedded policy is parsed once. The master overlay generation is 9.

## Patch fixes

- Settings UI resolves IL2CPP exports against Unity's loaded library handle,
  not only the process-global symbol scope. Missing classes/API are logged.
- A successful memorial delivery refreshes the stock mailbox on closing
  Settings. A weak managed handle avoids retaining a destroyed mailbox scene.
- The tested daily SubscribeList window correction is ported to the shared
  native core. It uses the launch device UTC offset, not a fixed Brisbane
  offset, and preserves the server-selected season.
- The tested follower-quest received-state correction is ported. Every server
  update replaces all three flags; another USN cannot read the cached flags.
- Both the ordinary rewarded-ad entry and the stock unavailable-ad branch
  invoke the original local success callback without the countdown dialog.
- New target instructions and field offsets were checked against the pinned
  8.0.0 input. The APK pipeline checks the input fingerprints before building.

## Verification performed

- Rust workspace: 77 passing tests; clippy with warnings denied.
- Private-fixture master parity: 54,371 scalar fields in 38 static tables match
  between Patch output and Server projection. SubscribeList is excluded because
  it is a per-save/day overlay, not a static table replacement. The check does
  not establish all RPC semantics.
- Patch Python suite: 20 passing tests.
- Android ARM64 standalone and development adapters compile with warnings
  denied. Required weak-handle and unboxing exports exist in the pinned input.
- Independent ARM64 test executable ran on the designated phone: positive and
  negative date boundaries, timezone-dependent day selection, independent claim
  flags, stage reset, and USN separation passed. This tests the pure native
  rules, not the full Unity scene.
- rev62 installed over rev61 without deleting data. A pre-install private app
  backup is retained outside the clean repositories. Startup Activity survived
  launch, but the secure keyguard prevented pressing its Start button.
- rev63, including the recovered daily-pass/quest/unavailable-ad hooks, then
  installed over rev62 successfully. Package revision is 800063; startup PID
  was 30823. The private XAPK SHA-256 is
  `5A1CC9679A15ECD2171D2B680CDA8C70953A493B03CFDFDEF344939484FADF94`.
  The first rev63 install attempt lost the host ADB connection and did not
  change revision 800062. Retrying with the executable used by the existing
  host daemon succeeded; no daemon or phone restart was requested.

Replay the private master test by setting `GGFM_ORIGINAL_MASTER` and
`GGFM_PATCHED_MASTER` to databases extracted from the user's verified package:

```text
cargo test -p ggfm-transport-http static_master_bundle_matches_server_policy -- --ignored --nocapture
```

## Runtime gates still open

### Resume checkpoint: rev65–71

- rev65 fixed the transformed master bundle's metadata as a single pipeline
  operation. The table cache hash, child manifest CRC, root manifest entries,
  size table and associated manifests now agree with the emitted bytes.
  rev65, rev67, rev68 and rev69 reached the game without the recurring 7.83 MB
  download or error 90005; app data/cache were retained. rev66 was not installed.
- rev68 showed a second, independent lifecycle defect: the first music bind
  succeeded, then late full login responses reconstructed records with null
  master references. A bounded read-only inspection found valid music IDs 1
  and 201, both with null master pointers. The catalog contained both IDs.
- Stock `Load(List<userMusic>, bool)` at RVA 0x1F53BC0 uses its boolean for
  immediate rebinding, **not merging**. Its true branch calls virtual slot 7.
  `Load(List<userCostume>, bool)` at 0x1E3E824 follows the same contract.
  Full login uses false because the first response precedes InitTableData.
- rev69 adapts those two loaders to the stock true branch only when the live
  TableManager has completed InitTableData (`m_isInit`, +0x45), and the instance
  is that manager's active music/costume table (+0x110/+0xF0). Cold-start still
  defers. No content or master pointers are fabricated; no ready flag or player
  object is cached across USNs or manager recreation.
- rev69 installed as an in-place three-split upgrade (versionCode 800069).
  Device logs show late `refresh=0 effective=1 tables_ready=1 success=1` loads
  and successful master bindings afterward. The Music page now renders its
  list, levels, costs and encore duration. Costume and Guitar pages rendered;
  visible non-default entries showed 30% / 20%. This is a visible sample, not
  an exhaustive all-content/CH2 acceptance claim.
- In-game test on the preserved memorial slot: Lily upgraded from level 1 to
  3; tap production changed 30 to 34. rev70 cold-start regressed to level 1.
  Fans remained zero and are still an open diagnostic, not a confirmed fix.
- Shop still reproduced a null reference in UserAdLevel's progress getter
  (0x1E0B528). Its list loader (0x2018138) also has the deferred-binding flag.
  rev70 source extends the same phase-aware adapter to the active ad-level
  table (+0x230). rev70 installed and the Shop list rendered normally, including
  free-reward progress entries. This is not purchase/ad callback acceptance.
- Separately, the retired price-localization Activity still produces empty
  SKU-list index errors. The tested LSPosed Java adapter exists under
  `reborn-hook/module/smali/gg/reborn/hook/PriceLocalizationHook.smali`; it has
  not yet been fully ported to the standalone runtime. Do not conflate this
  callback issue with the UserAdLevel master-binding error.
- Settings Show, OnEnable and auth hooks execute, but the entry is invisible
  despite UILabel updates. Actual runtime method pointers matched the pinned
  RVAs. rev70 adds bounded hierarchy diagnostics to distinguish inactive
  ancestors and overlapping platform rows. Runtime proved both Android and iOS
  online containers active at local `(270,-98)` under the same Support parent.
  Thus Restore Purchases covers Login; label assignment alone cannot fix it.
- Patch Python suite: 24 passed from the Patch repository working directory.
  Running discovery from the umbrella directory initially failed two imports;
  no test code was altered to hide that invocation error. Native libraries
  compile with warnings denied; the ARM64 pure-rule test also passed on the
  designated phone, including cold-start/late-load guard truth-table checks.

### rev71: upgrade upload and settings overlap repair

- rev70 reproduced level loss and the app's own atomic SQLite backup retained
  character 1 at level 1 after an on-screen level 3 upgrade. A successful older
  `userSave` was logged, but no subsequent upload of the upgrade was observed.
  Private backup: `build/rev70-before-upgrade-backup.sqlite`. This demonstrates
  that server flush alone does not collect Unity's unsent snapshot; it does
  not prove every possible automatic-save route is broken.
- New `client_save.cpp` wraps the stock successful character/follower upgrade
  operations. It keeps original qualification, deduction, return value and
  options, then requests `UploadSaveData(ALL=1, ..., useWaiting=false,
  keepWaiting=false, forceUpdate=true)` on the Unity thread. Including the
  complete snapshot pairs levels with currency deductions in the existing
  SQLite transaction. No live manager or player pointer is cached.
- Pinned entrypoints: character 0x1CB3890, follower 0x1CACA4C, DataManager getter
  0x21A6B94 and upload 0x2061654. Each has a verified 16-byte prologue in the
  compatibility manifest. The character disassembly returns false before
  successful deduction/mutation; the wrapper never uploads a rejected action.
  The upload call queues the request; it is NOT an acknowledgement of commit.
- Settings now hides the overlapping iOS restore container in normal mode.
  In the expanded memorial menu it places that container at the live login
  column / Privacy row world position. This reuses existing components and
  does not store a prefab or assume screen pixel dimensions.
- rev71 built, signature-verified and installed without deleting data. Process
  6692 survived startup and reached NEOWIZ. Before post-start interaction could
  be verified, the phone foreground moved to another app and subsequently to
  the ORIGINAL package. Further taps were paused; original app was not operated.
  Upgrade commit/restart and visible menu navigation remain UNVERIFIED on rev71.
- Native build passes with warnings denied. The ARM64 pure test ran on the
  phone and passed save ordering/rejected-action behavior, late binding guards,
  date/claim rules and USN separation. After packaging, the identical inline
  success/queue branch was extracted into a tested helper and recompiled;
  the next package should include that source-only cleanup as well.

Private screenshots are under `build/rev69-*`, `build/rev70-*`, and
`build/rev71-*`. They and app logs are local evidence, not public resources.
Only `org.guitargirlresuscitation.memorial` on `10.0.1.24:35877` was operated.
No phone reboot, original-app changes, save deletion, commit or push occurred.

The repaired Music/Shop paths still need repeated late-response checks and a
separately created clean-slot test, beyond the successful visible samples.
Settings visibility, all menu depths, actual mailbox arrival/claim, restart
persistence, fan production, CH1/CH2 catalog separation, CH3 score previews,
guide-chain stages and pass rotation still require in-game acceptance. No
phone reboot, original-app mutation, repository commit or push was performed.

The current native date correction uses the device offset captured at startup;
live timezone changes without a process restart require a separate lifecycle
check. Save switching must additionally prove completion of the outgoing
client snapshot before exit; SQLite durability alone is not proof that an
unsent Unity snapshot reached the server.

### rev72–75: independent settings panel and loopback diagnostics

- User clarified that the stock Login slot should be a stable entry to a
  separate panel, not a four-row menu painted over the Settings page. The
  native menu's domain actions now render our resource-free Android Dialog
  in the existing Unity Activity. No Activity launch, WebView, copied prefab,
  APK resources, or external service is needed. Buttons return to Unity's
  thread with UnitySendMessage; returning and closing worked on rev72/74.
- First-open label overwrite was reproduced on rev72. `blueasa.UILocalize`
  Start delegates to its localization method; static Refresh also visits
  disabled instances. Set only our replacement label's `m_eString` to the
  verified no-translation value 0 and disable its localizer; other game text
  remains untouched. The new stock-settings full-refresh hook is also pinned.
- First-open language selection incorrectly read UIToggle.mIsActive before
  Start. The actual `get_value()` selects startsActive until mStarted is true
  (0x1E4C7E0). Use that getter instead. rev74's FIRST opening showed Chinese
  “纪念版功能” directly in the Login slot, without auxiliary taps.
- rev72 admin listing failed to connect while the same PID's loopback listener
  remained alive. Network guard omitted ::ffff:127.0.0.1, used by Android Java
  dual-stack sockets. rev74 permits exactly that mapped address in addition
  to 127.0.0.1/::1, keeps LAN/public destinations denied, and bounds rejection
  diagnostics. The identical GET reached Axum after this fix. ARM64 tests cover
  mapped/unmapped local/public/LAN addresses and truncated sockaddr lengths.
- Then the successful admin GET exposed an empty catalogue: acquisition type 3
  is Star Pass, not retired events; those rows were then excluded as protected.
  rev75 moves the 29 audited old-Rust discontinued IDs into the shared policy
  manifest. The user's master still gates presence/active/area, and current
  CH3/pass rewards remain protected. No original names/assets are copied into
  this list. Normalized master namezhchs/nameen become zh-Hans/en API keys.
- Two policy copies match SHA256
  `758600A4E663D6C0B1148BABB412CE0719818ECE1C7FFBD3B8A135F9ED9E2893`.
  Six policy tests and 33 transport tests pass, including the new catalogue /
  localized JSON contract test. One asset-projection test is intentionally
  ignored without its external original/patched-master inputs. 25 Patch
  Python tests pass; native ARM64 compatibility tests pass on the phone.
- A new bounded PID-only capture helper uses a PTY and flushes each line to a
  private host log. This avoids the phone's small global logcat ring wrapping
  before post-hoc reads and avoids Android logcat-file block-buffer delays.
  No global logcat clear or unrelated app capture is performed.
- ERROR_100001 was seen on rev71 over Shop before the change. The listener and
  request hook were alive. One 525-chocolate claim succeeded on rev71; free
  chocolate (+10) and skill 1 upgrade (1→2, CP 4135→4126) succeeded on rev74.
  No request rejection was captured for those successful samples. This is
  NOT proof that every 100001 path is fixed. Repeated publisher re-login and
  a buff DateTime lookup NullReferenceException remain separate findings;
  the exact failed action still needs reproduction.

Private evidence: build/rev72-*.png, build/rev74-*.png and
build/rev74-runtime-private.log. rev72/74 were installed in place with the same
certificate, without deleting saves. rev75 catalogue/mail device acceptance
is pending at this checkpoint. No commits, pushes, phone reboots or original
package operations were made.

### rev76 source checkpoint: baseline is NOT achieved

The user reproduced paid/free-like Shop ERROR_100001 with accumulated likes
disappearing, no fan growth, achievements differing from the tested build,
and subsequently character upgrades failing. These are release blockers, not
cosmetic issues. No claim of gameplay parity is justified by the passing
fresh-login wire tests or a few earlier successful purchases.

The rev75 PID capture shows repeated membership callbacks followed by two
userLogin requests (identity-only then full state). It also shows a skill
manager DateTime lookup NullReferenceException. The lookup at 0x1BAE380
dereferences each skill record's master pointer at +0x98; later full loads
must not leave such references unbound. This is a concrete dependent failure,
but the exact trigger of the user's 100001 still needs a captured failing
Shop transaction. Do not replace all errors with successful responses.

One underlying omission is now repaired in source: the standalone membership
replacement previously invoked success without setting the publisher/game
login state. Stock IsLoggedIn (0x1E22254) checks ppMemberId, and the stock
commit method (0x1E217D0) writes ApiServer login type, guest status, member ID,
token, conflict ID and PPLogin. The replacement now obtains the active slot's
numeric userId from the authenticated local session endpoint, verifies its
USN, sets the corresponding membership fields through IL2CPP field APIs,
invokes that stock commit, checks IsLoggedIn, THEN invokes success. Failure
takes the failure callback. No fixed user ID, remote credential, or HTTP
capability is substituted into the publisher fields. Both methods' original
instruction fingerprints are pinned in the compatibility manifest.

Native compilation and bootstrap DEX compilation succeeded. A dedicated
ARM64 executable compiles the actual replacement with a fake IL2CPP boundary
and passed seven cases on the phone: valid identity, absent identity, missing
field, commit exception, failed logged-in check, invalid USN, and a different
USN (no cached identity). This is boundary-unit evidence, NOT game acceptance.
rev76 has NOT been packaged/installed; rev75 remains installed. Do not ask the
user to treat it as a fixed baseline. No saves were cleared or overwritten.

The rev75 retired Google local-price Activity also took foreground focus over
the visible game. The old PriceLocalizationHook's complete callback behavior
has not yet been ported. Keep this separate from network/database errors.

Pending user requirements (2026-09-05):

- Memorial items must be a scrolling list, with owned/pending-mail status and
  unavailable repeat grants, not previous/next pagination. Server listing
  currently lacks ownership projection; do not claim this is done.
- One readable AIRISUTEK ASCII-art banner, not letters forming themselves.
  Real English diagnostic logs, colored INFO/WARN/ERROR, terminal typography,
  and slower clearly separated ceremonial lines. Runtime hooks are only
  armed before Unity loads; do not label them installed prematurely.
- Startup lower panel: adjacent database export/import buttons, Android
  directory permission, Documents/<applicationId>/saves, actual destination
  confirmation. Import must happen with the server stopped, validate before
  replacement, and explicitly warn that ALL existing saves are overwritten.
- User explicitly overrides the proposed import backup: **NO automatic
  backup on import**. Do not silently create a rollback copy. File validation
  and an explicit destructive confirmation remain required. Import/export
  implementation and acceptance are still pending.

Startup logging source changes following that checkpoint:

- Real master validation/loading stages report schema, table count and content
  counts only after successful load. Policy projection and SQLite open each
  have explicit failure messages. SQLite diagnostics query actual schema,
  journal mode, foreign_keys and synchronous settings. Listener logs give the
  actual loopback endpoint, never the capability. Session activation reports
  its own begin/success/failure; no ceremonial line substitutes for these.
- Boot queue labels INFO/ERROR. Startup TextView renders INFO blue, WARN amber,
  ERROR red, scene text green, in monospaced text with ligatures disabled.
- One 35-column five-row AIRISUTEK banner uses '#' glyphs, with a plain-text
  AIRISUTEK caption. Milestone pauses are 400 ms; command/output split is
  350 ms with 650 ms before the next command; the welcome retains one second.
- Corrected an inaccurate startup claim: the pre-Unity stage arms an observer;
  it does not claim all gameplay hooks are installed before IL2CPP loads.
- 35 transport tests pass, one external-asset test remains ignored. Added
  negative startup test proves a missing master does not create a player DB
  or emit false storage/listening success. A real in-memory SQLite diagnostic
  test verifies the reported settings. Pure Java banner/severity tests pass,
  and the current bootstrap DEX is build/bootstrap-java-rev76-ui/classes.dex.
  Android server and native Patch compile successfully (jobs <= 2).
- The phone was in another user app when checked, so no foreground takeover,
  install or game mutation was made. Current installed rev75 remains known
  broken. Source rev76 requires a separate controlled game acceptance run.

### rev77 / rev78 continuation: direct Like purchase drift reproduced

- rev76 was packaged (SHA-256
  `5F2B107CDB92AAAB2AD45901976A39A7C5007800FE53FF3B210C6809CBC585B4`),
  but was not installed. rev77 was installed in place over rev75; startup
  Activity PID 13080 was observed. Unity/gameplay acceptance was NOT performed:
  the user returned to their other app. Do not equate installation with parity.
- Added `shop_baseline_tests.rs`: 41 consecutive claims for each of the three
  free products match the tested Rust response tree/scalars through level
  transitions. Retries do not change snapshots. A second SQLite connection sees
  committed data, and a second initialized USN stays unchanged. This is a
  connection-reopen test, not a process-kill test.
- The user explicitly reiterated that DIRECT Like purchases, not only free
  claims, fail. Extending the oracle chain to the active direct products 13
  and 20 reproduced a real failure: the new buyShop response omitted field 4
  `User_ad_level`, while the tested Rust returns the current 210010 row even
  for non-AdLevel products. Source rev78 restores that projection in the same
  actor job as the purchase, without incrementing its level/EXP. Three direct
  purchases per product now match the old wire trees/scalars. Repeat delivery
  leaves balances unchanged; direct claims do not change any free-product EXP.
  This proves a response defect, NOT yet the device ERROR_100001 root cause.
- Do not claim fan growth or character upgrades recovered. They remain explicit
  device acceptance blockers together with direct purchases and re-entry saves.
- Memorial catalog now projects owned/pendingMail/canSend per USN. Catalog and
  enqueue share the same ownership guard, including area=0 reward ownership,
  which previously let a chapter-specific mail bypass an existing grant. Added
  a regression for pending mail, unscoped ownership, and separate USNs.
- rev77 Patch changes replace legacy Prev/Next pages with a scrolling item
  list. Unavailable items use the existing localized owned-or-pending label and
  remain disabled. Successful enqueue refreshes the list and shows a message.
  A generation-tagged atomic queue transfers Android selections to Unity;
  stale-page and duplicate clicks cannot become other menu actions. The actual
  ARM64 selection queue executable passed on-device without operating the game.
  Visual and game mailbox acceptance remain pending.
- User rejected the hand-drawn banner and requested a generator. rev78 uses
  pyfiglet 1.0.4 standard-font AIRISUTEK, generated by
  `tools/generate_terminal_banner.py`. pyfiglet is installed only under the
  private workspace build/figlet-tool; no runtime dependency/fonts are shipped.
  GATE 07 and HOME footers are removed per the latest request. Banner-only size
  fitting prevents wrapped ASCII at larger system font scales. Java text/layout
  tests pass; current DEX is build/bootstrap-java-rev78-figlet/classes.dex.
- Latest server unit run: persistence 28 passed; transport 36 passed, 1 private
  fixture test ignored. Patch Python 25 passed; native and DEX compile passed.
  These counts do not represent 62 fully accepted gameplay handlers.
- The user explicitly authorized a future uninstall/reinstall of ONLY
  org.guitargirlresuscitation.memorial to discard the suspected corrupt test
  saves. At this source checkpoint it has NOT been uninstalled; rev77 data still
  exists. Original com.neowiz.game.guitargirl must remain untouched. No phone
  reboot, Git commit or push is authorized. Import/export remains unimplemented;
  the separate instruction of no backup on import still applies.

### rev78 package and clean-install checkpoint

- Package build/ggfm-e2e-smoke-78.xapk SHA-256:
  `45A41193A93BA9BE0533FF52423556FF4906E5F9137B41FEA34AE96EF402AE2C`.
- Extracted only table_db.ab from the FINAL base_assets split using a disk-backed
  temporary APK, then regenerated build/rev78-patched-master.sqlite. The opt-in
  master projection test passed: 54,371 scalar fields across 38 static tables
  matched the server projection from build/master-rev53.sqlite. This is final
  package evidence, not a comparison against an old intermediate artifact.
- Verified installed target package paths/version 800077, then performed the
  user's explicitly requested uninstall of org.guitargirlresuscitation.memorial
  WITHOUT -k. Installed rev78 successfully; verified versionCode=800078 and
  startup Activity PID 29642. No backup of the just-removed test app data was
  made. The original game package was not modified and the phone was not rebooted.
- Device was Dozing; KEYCODE_WAKEUP woke the display without rebooting it.
  KeyguardServiceDelegate reported showing=true and KeyguardStateMonitor
  mInputRestricted=true. Do not dismiss/bypass secure lock or claim gameplay
  testing: the user must unlock before Start, the tutorial, direct purchases,
  fan growth and upgrades can be exercised. PID-filtered log capture was stopped.
- Actual on-phone acceptance of the FIGlet art, scrolling legacy list and
  membership fix is still pending. Startup PID alone does not prove any of them.

### rev79 / rev80: login telemetry, invalid reference writes, terminal changes

- The rev78 lock-screen blocker ended when the user unlocked and pressed Start.
  Reproduced ERROR_20001: GET /memorial/v1/session succeeded, followed by
  Firebase internal::IsInitialized() and local membership committed=0.
- Pinned stock membership commit calls the member-ID setter, which calls
  FirebaseManager.SetFirebase_UserId (0x2214B48), analytics and Crashlytics.
  rev79 retires only the three verified telemetry sinks SetUserId (analytics
  and Crashlytics) and SetUserProperty; their 16-byte prologues were checked
  against the original ELF. Stock identity commit and IsLoggedIn remain active.
  All failure branches now log the failing stage without credentials or IDs.
- rev79 installed in place. The user reported a crash after the headphones
  recommendation; captured SIGSEGV on AssetGarbageCol, PC 0x1A4CFEC, invalid
  class pointer 6 while walking a managed object. This is NOT proof of a bad
  asset bundle or a reason to disable GC.
- Found an actual Patch bug: il2cpp_field_set_value at 0x1A0AB34 branches to
  0x1A60304, which invokes SetValueRaw at 0x1A5FC4C with deref=false. The
  reference-type branch passes the supplied object directly to the write
  barrier. Our three string writes supplied &pointer (stack storage) instead
  of pointer. rev80 fixes those writes, preserving &guest for the boolean.
  The fake boundary test had encoded the same wrong assumption; corrected it.
  The rebuilt production-replacement test passes on phone (7 scenarios), but
  that alone does not establish that the real crash is gone.
- Native Rust startup renderer now owns a minimal ANSI Shadow glyph subset.
  A new additive C ABI accepts available columns: >=66 gives AIRISUTEK in one
  line; otherwise AIRISU / TEK, each <=46 columns. Tiny displays use a separate
  horizontally scrollable banner rather than wrap glyphs or shrink text.
  Actual width is measured using the font, viewport, padding and system font
  scale, not a device-model DPI guess. Explicit DroidSansMono loading avoids
  OEM theme replacement when that system font is available.
- Banner appears only after Start and remains for 900 ms before boot begins.
  Cosmetic commands include parameters, reply after 150 ms, then a blank line
  and 1100 ms before the next command. A bare purple divider animates '.', '..',
  '...' in place before Welcome home, Lily!; final welcome stays 1000 ms.
  Cosmetic commands are not shell commands and are not executed.
- Two Rust banner tests pass (66/46 layout bounds, UTF-8 C ABI capacity and
  no partial writes); Java ritual timing/style tests pass; Patch Python tests
  remain 25/25. ARM64 native and Android Rust release builds pass.
- rev80 package/build and on-phone cold-start verification were performed;
  it reached the Joie tutorial. This establishes that startup passed in that
  run, not full gameplay parity. XAPK SHA-256:
  `15D3909DBF56ADB90A860759803C437A044D18A54F3B87F8DB1755620CBE2DF2`.
  Do not mark fans, upgrades, paid Shop or full gameplay parity as accepted.
  No further uninstall, phone reboot, commit or push was performed.

## Offline batch checkpoint (after rev80; not installed)

Latest continuation: see `ACTIVITY_CH3_OFFLINE_REGRESSIONS_20260905.md` for
Samseck login restoration, CH3 deadline/score fixes and current test evidence.
The user subsequently authorized the USB Samsung after static checks; the
older no-phone instruction below describes the earlier away period only.

The user is away and the phone must not be accessed. Keep all further tests
local and single-heavy-worker. No rev81 package has been built or installed.

Source already added earlier in this batch:

- Offline SQLite export/import via SAF with lower startup buttons, localized
  overwrite warning and no automatic import backup. Rust transfer tests pass;
  Android UI/SAF behavior is unverified. Review revoked tree grants and cache
  clearing failure after DB import before device acceptance.
- `TerminalBannerView` fixed-cell Canvas rendering to address fallback glyph
  width; compiled DEX is `build/bootstrap-java-rev81-transfer-grid/classes.dex`.
  Visual result remains unverified.
- Attendance achievement ID 10 excluded from 0.1x policy in both manifests and
  master transformations. Skill unlock requirements retain original levels.
- Achievement request quantity accepts `S_quantity`; chocolate reward grant
  and DailyMission alias added. Additional wire-focused tests still needed.
- Server-owned mail keys localized at projection, including existing Samseck
  messages. This does not yet include the requested multiplier-specific copy.

This increment:

- Confirmed client Consume semantics and documented them in
  `CONSUME_REWARD_CONTRACT.md`; fixed gameplay-mail ID/channel loss.
- Fixed attendance claim verification so an inflated persisted achievement
  counter cannot override distinct-date attendance. Regression covers repeated
  login/attendance on the same date, the third distinct date, grant and replay.
- Local test command: `cargo test -p ggfm-domain -p ggfm-persistence-sqlite
  -p ggfm-transport-http --lib -j 2`: 1 domain, 31 persistence and 37 transport
  tests passed; 1 private master-projection test ignored.

Important remaining work (do not label this batch complete):

- Shared multiplier resolution is still missing in persistent reward credit;
  use the new contract, not the old fixed fan-mail test, as semantic evidence.
- Currency list/input, localized unit explanation and send-crash isolation.
- Skill first-level double-click/auto-unlock reconciliation.
- Shop/Fever stutter and retired price-localization callback parity.
- Rebuild the patched master with the new policy and run the full private
  master-field comparison; old rev78 patched master is now a stale fixture.
- Rebuild Android Rust/native/DEX together with refreshed hashes, then package.
- Import/export, banner and all gameplay regressions still need device testing
  when the user returns. Preserve existing data; do not uninstall again without
  renewed scope/confirmation, and never reboot the phone.

## Full guide / CH3 reload continuation

See `ACTIVITY_CH3_OFFLINE_REGRESSIONS_20260905.md` for the newer evidence:
42-stage / 126-task master-backed guide regression, completed-stage projection,
Fever timer persistence, CH3 entry-prefix wire parity, and restored profile,
gift and individual affection-claim login lists. Development schema is now 4;
there is no test-save migration or automatic erase. This remains a static
checkpoint, not a claim that the reported device CH3 crash has been resolved.
