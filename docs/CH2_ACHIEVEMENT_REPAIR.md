# CH2 achievement catalog repair

The CH2 achievement UI filters master groups 0/2 and only creates rows whose
IDs exist in the player's achievement dictionary. The previous slot seed
hardcoded IDs 1..10, omitting active CH2 IDs 201..207.

Startup now configures the active achievement IDs from the mounted master
catalog, before creating a default slot. One transaction inserts missing rows
for every existing USN with `ON CONFLICT DO NOTHING`. Existing quantities,
timestamps and claims are never replaced. New slots use the same configured
catalog. No old save must be deleted or reimported.

Regression covers two old slots, progress and claims, new-slot creation,
repeated startup and reopen. A private coherent backup from the affected
installation was also run through the actual desktop Server twice: 10 rows
became 17, all five claims and all existing achievement rows were unchanged.
This check did not modify the phone. The affected phone still needs an
identity-compatible upgrade and a cold start before its UI receives the repair.
