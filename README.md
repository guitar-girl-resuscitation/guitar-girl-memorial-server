# Guitar Girl Fan Memorial Server

Rust interoperability implementation of the local, built-in server for the Guitar Girl Fan Memorial Build, informed by client analysis and protocol evidence.

The legacy `reborn-server` is protocol evidence only. New behavior belongs here after it is expressed as a documented contract and a regression test.

## Non-negotiable invariants

- One save slot has one immutable positive USN (`U_seq`) and one immutable numeric-string `U_id`.
- Every player-owned database row is keyed by USN. No global player state exists.
- A slot switch is pending until the client returns to the title/login flow. An active RPC session never changes USN.
- Calendar behavior derives from device Unix time plus the device UTC offset supplied on launch. The server need not remain alive while the game is closed.
- Mutations are transactional and reject an identity different from the session USN.

Run the core tests with `cargo test`.
