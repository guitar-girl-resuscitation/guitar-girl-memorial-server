# Device time audit (2026-09-08)

## Clock contracts

- Attendance, daily missions and pass rotation use DeviceClock.local_epoch_day:
  floor((Unix seconds + current device offset) / 86400). Server restart is not
  a new day. Existing date keys make an already claimed local date idempotent.
- Pass selection persists an anchor date/season per USN. Re-login does not
  reset the anchor. User subscription activation is device midnight expressed
  as an absolute Unix instant.
- CH3 energy, skill timers, music encore deadlines and Fever use absolute Unix
  seconds. Offset must not be added to stored deadlines or elapsed durations.
- Offline rewards retain the client's stock hibernation clock. Both endpoints
  use the same stock UTC+9 DateTime representation, so subtraction measures
  elapsed time; local timezone must not be added again. Samsung reopening
  displayed an offline reward during the UTC-6 test.
- Mail creation uses Unix seconds. Memorial reward mail is unlimited, not
  subject to a local-midnight expiry or per-timezone reward reissue.
- Guide progress/claims are stage-and-USN scoped, not date scoped.

## Verification

Persistence tests cover ten offsets including UTC-6, half-hour and quarter-hour
zones: repeated logins on one local date count once, midnight advances once,
and pass anchors remain unchanged. Spring-forward/fall-back repeated/skipped
hours do not produce an extra attendance day. Existing Fever/energy cold-reopen,
music timestamp preservation, mail idempotency and guide restart tests pass.
Transport tests cover all 13 pass seasons over two rotations and nine offsets.
Workspace tests pass (one pre-existing private master-overlay fixture ignored).

The Patch separately converts device midnight to the stock UTC+9 comparison
clock for pass visibility. Samsung UTC-6 cold-start/UI checks passed; native
request offset updates were also observed in a running process. This does not
mean every time-based UI was manually tested across every timezone. Cached pass
UI still requires cold reopening after a live device timezone change.

Deliberately changing the device calendar across dates can change the season
and make a previously unclaimed date eligible, consistent with offline device-
date policy. No anti-time-cheat or migration was added. Existing saves retained.
