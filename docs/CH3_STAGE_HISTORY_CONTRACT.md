# CH3 stage history versus eligibility

The login `User_chthird_stage` and `getChThird` `User_ch_third_stage`
collections are completion history, not a list of the next playable nodes.
SQLite retains separate `unlocked` and `completed` states.

Client 8.0.0 static evidence:

- `GgChThirdManagerSGT.GetIsAvailableStart` at RVA `0x1D39398`
  accepts stage index 1 directly and otherwise checks the preceding stage
  index in the chapter's user-stage dictionary.
- `GetIsClearStage` at RVA `0x1D391A0` first requires a user-stage row.
  Normal stages require positive stars; story stages are considered clear
  from the row's presence, even with zero stars.

Consequently, publishing an unplayed story row incorrectly advances the
client's map. Publishing the next unlocked node can also expose a further
node which the server correctly rejects as locked.

Both snapshot response paths use `ch3_completed_stage_values`. A fresh save
has internal 1001 unlocked but an empty wire history. Clearing 1001 publishes
1001 and internally unlocks 1002; clearing 1002 publishes both completed rows
and internally unlocks 1003. Failed stages do not extend completion history.
Do not send only the latest completed row: earlier progress and star totals
must survive login.

This corrects a behavior in the old test server which seeded the initial
story and included the next unplayed node in its projected list.

Regression coverage:

- `ch3_progression_requires_each_previous_stage_and_failure_does_not_unlock`
- `ch3_wire_reports_completed_history_not_the_unplayed_frontier`

Samsung device acceptance on 2026-09-06: 1-1 is a live stage, not a story.
A fresh map showed only 1-1; its 187-point three-star clear unlocked 1-2.
Completing story 1-2 unlocked story 1-3, and completing that unlocked only
1-4. A same-signature upgrade and cold reload preserved all three completed
nodes, the three-star history, and the 1-4 frontier. Failure behavior is covered
by the regression test above; the one-time map prompt is a separate UI contract.
Existing player databases are not deleted or reset by this change.
