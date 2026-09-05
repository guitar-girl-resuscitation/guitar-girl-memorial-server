# Architecture baseline

## Runtime

The patch starts the Rust server inside the application process, passes a device clock snapshot and a process-generated capability, and waits for a loopback-ready signal. The release build has no sidecar or external-server mode. Closing the game stops the server. No timer, daily job, or shutdown callback is required for correctness.

## Identity and saves

USN is the primary identity, not a field inside a mutable JSON document. `U_id` is a second immutable identity and must agree with the session. Device and platform UUIDs identify the installation/login source; they do not partition player state.

A session captures `(usn, user_id, session_nonce)` at title login. Every mutation executes against that captured USN. Requests carrying another identity are rejected before decoding business fields.

Slot switching is a two-phase operation:

1. Settings writes `pending_usn`.
2. Patch returns to the title and tears down the gameplay session.
3. The next login atomically moves `pending_usn` to `active_usn`, increments `session_nonce`, switches client-local namespaces, and performs the normal `U_seq=0 -> assigned USN -> full login` sequence.

This prevents a late autosave from the previous scene writing into the newly selected slot.

## Storage

SQLite is the authority. All player-owned tables have USN in their primary key or as their primary key. Foreign keys and immutable-identity triggers make a missing scope a database error rather than a coding convention.

There is no whole-player JSON authority or cross-slot fallback. All implemented protocol areas write normalized USN-scoped tables; opaque JSON is permitted only inside an append-only transaction summary where `json_valid` is enforced.

The writer uses WAL, `synchronous=FULL`, foreign keys and an integrity check.
On clean shutdown it creates a self-contained `VACUUM INTO` snapshot before
publishing it as the current backup while retaining one previous generation.
If the primary database fails its integrity/open checks, startup preserves the
primary and its WAL/SHM sidecars under a `.corrupt.<time>` suffix and tries the
two consistent snapshots. It never treats a copied live main file without its
WAL as a backup.

## Device time

The patch supplies Unix seconds and the device UTC offset at each launch and whenever Android reports a time-zone change. Calendar functions derive the local epoch day from that snapshot. Persisted anchors store a local day, so the process may stop for weeks and deterministically compute elapsed days on the next launch.
