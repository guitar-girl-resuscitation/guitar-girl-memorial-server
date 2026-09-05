# Source and binary releases

The public tree contains authored interoperability code, value-free protocol
schemas and synthetic tests. Private baseline-oracle tests, original packages,
game master rows, raw captures, user data and reversing exports stay outside it.
Passing public tests does not claim a new device gameplay acceptance run.

Run `python tools/release_guard.py --history` after staging. The guard checks
the exact index and every reachable historical blob, including file types,
binary magic, copied-code markers and key formats. A human provenance review
remains necessary; pattern matching cannot certify copyright ownership.

Every main-branch change builds the Linux CLI and Android ARM64 C ABI, using
Cargo's lockfile and two compiler jobs. Outputs contain only approved names,
the license, a file-hash manifest and exact source commit. No master database
or original or patched game package is a build input or Release asset.

Only `nightly` is replaceable: after tests and builds pass, its old Release and
tag are deleted and recreated at the built commit. Obsolete queued builds do
not replace a newer main-branch build. Version tags (`v0.1.0`, etc.) are immutable.
If any build fails, the last working Nightly remains available.

Patch must be rebuilt against the selected Server binary and policy hash.
Android's public SONAME is `libggfm_server.so`; Cargo's intermediate output is
`libggfm_android_ffi.so`. The release archive uses the public name.
