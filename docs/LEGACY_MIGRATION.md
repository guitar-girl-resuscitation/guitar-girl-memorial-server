# Legacy migration map

The old implementation remains in `../reborn-server` during migration and is never used as the new persistence layer.

| Legacy area | New owner | First invariant |
|---|---|---|
| Thrift schema and wire codec | `protocol` module | Captured request/response fixtures decode byte-for-byte |
| `PlayerState` JSON | SQLite slot repository | Every row is keyed by immutable USN |
| Login and choice-user RPCs | identity/session service | No full player data is returned under a mismatched USN |
| Mail | USN-scoped mailbox | Claim and grant commit in one transaction |
| Pass | USN-scoped pass tables | Season/version/lane/step are independent keys |
| CH1/CH2/CH3 progress | normalized progress tables | Client snapshots cannot roll authoritative progress back |
| Memorial panel | Patch UI plus local admin API | Every action names the active session USN |

Migration order: identity/session, wire codec, login snapshot, durable save, mail, purchases, progress systems, events/pass, CH3, then optional/telemetry RPCs.
