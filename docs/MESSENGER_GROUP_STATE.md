# Messenger group-relative progress

The v8 client changes a room's active conversation group independently of the
number of previously read lines. The last-confirmed index is **group-relative**,
not a lifetime maximum for the follower or room.

Static client evidence: `DoNextProgress` (0x1DA2784) enters `MoveGroup`
(0x1DA2B94). When choosing a new group, the client sets its current group and
resets the confirmed index to -1 before persisting the room state.

Consequently, `userSave` must replace the following fields together in a single
USN-scoped transaction:

- `last_confirm_index`
- `unlock_group_list` (client-owned serialized group state)
- `update_time_ticks`

Taking `MAX(last_confirm_index)` while replacing the serialized group mixes two
different conversations. Taking `MAX(update_time_ticks)` is also inappropriate:
device wall time may move backwards. Authenticated monotonic request sequences,
not these values, reject late writes.

The regression test exercises index 20 -> -1 -> 0, a backwards wall-clock step,
another USN with the same room ID, and a late old-group save with a newer clock.
This corrects persistence semantics; it does not itself prove that every
out-of-order follower unlock satisfies the stock client's dialogue prerequisites.
That behavior requires separate device acceptance.

The client's eligibility check (0x2242928) reads explicit master prerequisites:
CH1/CH2 character levels, the required follower and its level, an owned prop,
and completion of a required conversation group in its room. It does not compare
the historical order in which followers were bought. Preserve these prerequisites;
do not mark all dialogue groups complete to work around missing messages.

Samsung spot check (2026-09-06): Moa was bought before other followers, then
Teacher before Convenience Store Friend. Both Teacher and Convenience chats
became available after prerequisite dialogue. Joy group 1 and Convenience
group 2 were completed using actual replies. This is not exhaustive acceptance
of Jae and every later conversation chain; their level/group requirements remain.
