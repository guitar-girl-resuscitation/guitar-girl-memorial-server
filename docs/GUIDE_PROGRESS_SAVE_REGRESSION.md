# SupportGuitarGirl unclaimed task progress

The legacy baseline excluded all follower quest counters from userSave to avoid
cross-stage progress leakage. That also discarded legitimate partial progress:
local task events mark the follower-quest save partition, while claiming uses
setUserFollowerQuest. A player leaving before claiming could lose those counters
when the next login projects the server snapshot.

The merger now accepts finite nonnegative counters only when their CurrentID
equals the server-owned active stage. Counters merge monotonically; a save
cannot select a later stage, reset claimed tasks, grant rewards, complete a stage
or advance the chain. Claimed counters and completed stages are unchanged.
The existing authenticated monotonic request sequence rejects late writes.
Snapshots missing this partition do nothing. Reward grants remain transactional
and idempotent; all three claims are still required before progression.

Regression coverage includes partial progress, zero/invalid values, missing
partitions, wrong/prior stages, stale requests, repeat claims, cold SQLite reopen,
new login sessions, separate USNs and independently saved next-stage progress.
Existing four-stage per-task/reward restart tests also pass. This verifies the
server contract, not force-killing a phone before its pending save RPC commits.

Existing saves are retained. Already committed claims/stages need no migration.
Counters discarded by an older build cannot be reconstructed from this change
alone; future acknowledged saves retain them. A reported complete-stage reset
still needs its actual save/log to reproduce if it persists after this fix.
