# Daily pass and candy hotfix

This is an in-place update of the released schema-4 database. It changes no
schema, USN, PlayerPrefs namespace, package identity, claims or ownership rows.
Do not uninstall, clear application data, or initialize a replacement database
to deploy this fix. Keep the distributed application's signing identity.

## Pass subscription

The master response already calculated the device-local daily season from the
saved anchor. Login/load incorrectly filtered subscriptions using the anchor
season itself. After daily rotation these responses described different passes.
Both now use `pass_season` with the same request device clock. The subscription
activation time is the current device-local midnight, not an old anchor date.
The database retains all 13 entitlements and independent claims unchanged.

## Candy ownership

`buyShop.U_candy` is the total post-transaction balance: the client sets its
balance after processing the reward list. It must not contain the reward delta.
The shop implementation already reads the transaction total. The defect was a
second, stale CH1 `area_state.candy` value in login/load, plus `userSave` accepting
that client snapshot as a replacement for `currencies.candy`.

CH1 `Area_data.D_Candy` now reads the same authoritative currency as `User.U_candy`.
CH1 candy snapshots in `userSave` no longer mutate either authoritative balance
or its obsolete area mirror. CH2 area data is left unchanged. Real spending is
still deducted transactionally; rewards remain additive. In particular:

- Shop, passes, activities and generic rewards use `apply_reward_grant`.
- Mail uses `apply_stored_reward` and its claim transaction.
- Music encore adds its total to the existing currency balance.
- Neither zero nor large client snapshots are evidence of a candy reward.

Previously lost balances are not fabricated or globally reset. Existing server
balances, slots and claim ledgers remain intact.

## Regression evidence

- Subscription wire responses cover two daily cycles, all 13 seasons, and four
  device offsets including UTC-12 and UTC+14.
- Starting from 180 candy: chocolate-only shop preserves 180; two 30-candy
  purchases give 240; encore gives 247; mail gives 258; one paid claim in each
  season gives 648. Replaying each claim never grants twice.
- Saves containing 0, 30 or 9999 candy cannot overwrite that balance; a real
  10-candy cost reduces it to 638.
- SQLite backup/reopen preserves the entire resulting player snapshot and a
  separate slot remains independent. Schema version remains 4.

These are automated server tests, not a claim that an Android in-place upgrade
has already been tested on a player's device.
