# Android ARMv7 support status (2026-09-06)

## Implemented and verified

- Server builds sequentially for arm64-v8a and armeabi-v7a, using one compile
  job. C ABI version remains 1. CI publishes separate runtime archives.
- A Samsung SM-A336E ran the Server lifecycle harness in a real 32-bit process:
  two start/login/shutdown cycles, persisted USN, fresh session capability,
  loopback endpoint and SQLite integrity all passed.
- The complete game produced by the actual Patcher also entered the game room
  as primaryCpuAbi=armeabi-v7a, through login, opening chat and native notices.
  An update with the CH2 achievement repair entered the room again without
  deleting its test save. The app uses an isolated test package and signer.
- Patch independently maps 66 hooks, 10 native dependencies and 40 field offsets.
  PlayerPrefs.Save is left native. The pinned ARM branch-relocation correction
  has instruction-emulation tests and the real-device startup regression.
- Patcher validates ELF32/EM_ARM and ELF64/EM_AARCH64, dependencies, fingerprints
  and matching split/library paths. Automatic updates select the source ABI and
  reject mixed runtime artifacts.

## Source and distribution boundary

The original ARMv7 installed splits are version 8.0.0/code 800:

| APK | SHA-256 |
| --- | --- |
| base.apk | d770305126594048db05cc593c395761c20c06fc97ebfab88285df3998564f87 |
| split_config.armeabi_v7a.apk | 49848f553811a72379385a90ccc7cb3bbb0622fd39d13aaa757c1319071c69e6 |
| split_base_assets.apk | 5854156b052c6d7a8aaa8b920393741535ea7bcb993d3fa0be9b7d133d8e8886 |

The local V7 compatibility profile identifies a private container of those
verified original splits, not a public APKPure XAPK. See Patch
compatibility/ARMV7.md. Original game binaries are not included in releases.
The ARM64 APKPure source has no V7 game library; it cannot produce V7 alone.
Web deployments currently use one profile per deployment, not universal output.

## Build

```sh
python tools/build_android.py --ndk /path/to/ndk/27.3.13750724 --abi armeabi-v7a
```

Rust target: armv7-linux-androideabi. NDK prefix: armv7a-linux-androideabi.
The default ABI remains ARM64.

## Acceptance still required

The tested handset can execute 32-bit apps but is not the community's
32-bit-only Redmi A1. Full gameplay, CH3 and supported older Android devices
are not all accepted by a startup test. Preserve the published application ID,
signer and saves for upgrades. Do not distribute test-signed integration builds
as updates to published installations.
