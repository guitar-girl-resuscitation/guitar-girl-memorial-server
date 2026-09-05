# Consume rewards: mailbox, Shop and Star Pass

Status (2026-09-05): client semantics confirmed; mail ID preservation fixed in
source. Shared server-side multiplier settlement is NOT yet implemented and
must not be described as validated gameplay parity.

## Evidence boundary

The source is the user's verified 8.0.0 client, not the unfinished third-party
server. Private local evidence (not for redistribution):

- `build/master-static-rev62/original.sqlite`, table `Consume`.
- `analysis/full-rpc-audit/semantic-callee-decompile/semantic-callees-05.txt`:
  reward merger `0x1D917D4` and reward amount display `0x1D92BA0`.
- `analysis/full-rpc-audit/semantic-callee-decompile/semantic-callees-11.txt`:
  reward application calls the same quantity converters.
- `analysis/full-rpc-audit/activity-mail-decompile.txt`: providePost constructs
  reward objects from the returned type, ID and double quantity, then calls
  `0x1D917D4`.
- Bounded disassembly of the verified `libil2cpp.so` confirms the converter
  calls below. No new large IDA/Ghidra worker or phone session was started.

## Exact meanings

All rows below are reward type 1 (Consume). Do not infer units solely from
`Consume.i_Type`: the fan rows have type 0 but use multiplier applicators.

| Consume ID | Destination | Input quantity |
| --- | --- | --- |
| 1 | Chocolate | Count |
| 2 | Candy | Count |
| 3 | CH1 Likes | Literal count |
| 4 | CH1 Likes | Production multiplier |
| 5 | CH2 notes | Literal count |
| 6 | CH2 hearts | Literal count |
| 7 | CH2 hearts | Production multiplier |
| 8 | CH1 fans | Channel 1 fan-gain multiplier |
| 9 | CH2 viewers | Channel 2 fan-gain multiplier |
| 10 | CH2 notes | Production multiplier |
| 11 | CH3 cookies | Energy count, not a normal currency balance |

The merger resolves 4 -> 3, 7 -> 6 and 10 -> 5, calculates the resulting
BigInteger, then merges the resulting ordinary rewards. The converters are:

- CH1 `0x1D9205C`: combines `0x1CB00EC` and `0x1CB85D4`, then invokes
  `0x2226EA8` with the input multiplier and argument 2.
- CH2 hearts `0x1D92150`: uses `0x1CB6F3C`, then the same scaling helper.
- CH2 notes `0x1D921DC`: uses `0x1CB5EE4`, then the same scaling helper.
- CH1 fans `0x1D926C0`: channel 1 getter `0x1E3FC80`, float-to-double
  conversion, multiply. The applicator converts the result to an integer and
  calls the channel 1 fan update.
- CH2 fans use `0x1D93730` and the channel 2 update.

Do not describe fans as input times total owned fans. Bounded disassembly of
`0x1E3FC80` confirms a basis of `1.0f32`, plus each owned costume's bonus for
the requested area, using float32 additions. The converter promotes this sum
to double and multiplies the input; the applicator truncates to int32.
For example, one 30% outfit and input 1000 yields 1299, not a rounded 1300.
Keep owned outfit/guitar/other production bonuses: their inclusion is original
game behavior, not an error to remove.

The production getter names are `GetLikePerTapWithBuff`,
`GetLikePerSecWithBuff`, `GetHeartPerSecWithBuff`, and
`GetNotePerTapWithBuff`, in the order listed above. For nonnegative finite
inputs the precision-2 BigInteger scaling helper is equivalent to
`basis * trunc(multiplier * 100) / 100`, with integer division. The complete
with-buff production bases still need implementation and test vectors.

## Implemented this increment

- `domain::ConsumeReward` classifies all 11 IDs with their channel and unit.
- Gameplay-generated mail retains its original Consume ID and channel in
  `mail_rewards`; mailbox projection no longer reconstructs 7 as 6 or 9 as 8.
- Stored currency/ID mismatches and out-of-range IDs are rejected.
- Existing rows without an ID remain readable without silently rewriting saves.
- Tests cover IDs 1..10, repeated enqueue, USN isolation and invalid stored IDs.
- `RewardRules` is compiled from the master/policy projection at startup, not
  saved in player data. Consume 8/9 share transactional fan settlement in mail,
  Shop, Pass and the other reward commands; CH2 viewers stay in area 2.
- Non-unit bonus tests exercise Shop, Pass and mail together, both channels,
  replay, and another USN. Fan overflow/nonfinite values reject the transaction.
- Currency selection is now a full localized list with an input dialog in
  Patch source. The new DEX compiles; no device/package acceptance yet.

## Remaining work before packaging

The existing `apply_reward_grant` and `apply_stored_reward` still add production
multiplier IDs 4/7/10 as literal balances. Consume 8/9 have been corrected.
Preserving wire IDs alone does NOT prove correct persistent production credit;
the old Shop tests are not an arithmetic oracle for those remaining paths.

1. Implement/test the production bases, BigInteger scaling and numeric limits.
2. Introduce one shared resolution path for Shop, Pass, ads and providePost.
   Keep the wire reward and resolved credit distinct; resolve once from a
   consistent pre-grant snapshot inside the claiming transaction.
3. Retain the now-tested separation of channel 2 viewers and channel 1 fans.
4. Change Memorial mail input to multipliers for Likes, notes and CH1 fans,
   counts for chocolate/candy, with matching localized labels and mail copy.
   The current admin endpoint still constructs a plain `Reward::Currency`;
   this audit does not claim that endpoint has been converted.
5. Finish source-level list/input tests and verify the Unity-thread handoff on
   device; the backend endpoint must be converted before packaging this UI.
6. Test non-unit bases (not just basis=1), post-send production upgrades,
   display versus credit, replay, two USNs, restart and late userSave. Do not
   rely on a later client save to repair an incorrectly committed reward.

The historical mailbox notes describing fixed fan grants/current-balance
cards are superseded by this static evidence. Those earlier observations
were symptoms, not a confirmed definition of the reward unit.
