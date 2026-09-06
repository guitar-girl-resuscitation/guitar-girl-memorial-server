#!/usr/bin/env python3
"""Build the stable Android ABI with bounded parallelism and an explicit NDK."""
import argparse
import os
from pathlib import Path
import subprocess

p = argparse.ArgumentParser()
p.add_argument("--ndk", type=Path, required=True)
p.add_argument("--abi", choices=("arm64-v8a", "armeabi-v7a"), default="arm64-v8a")
args = p.parse_args()
root = Path(__file__).resolve().parents[1]
host = "windows-x86_64" if os.name == "nt" else "linux-x86_64"
bin_dir = args.ndk.resolve() / "toolchains/llvm/prebuilt" / host / "bin"
target, compiler = {
    "arm64-v8a": ("aarch64-linux-android", "aarch64-linux-android"),
    "armeabi-v7a": ("armv7-linux-androideabi", "armv7a-linux-androideabi"),
}[args.abi]
cc = bin_dir / (compiler + "28-clang" + (".cmd" if os.name == "nt" else ""))
ar = bin_dir / ("llvm-ar.exe" if os.name == "nt" else "llvm-ar")
if not cc.is_file() or not ar.is_file():
    raise SystemExit("NDK compiler missing")
env = dict(os.environ)
target_key = target.replace("-", "_")
env.update({"CC_" + target_key: str(cc), "AR_" + target_key: str(ar),
            "CARGO_TARGET_" + target_key.upper() + "_LINKER": str(cc),
            "CARGO_BUILD_JOBS": "1"})
subprocess.run(["cargo", "build", "--locked", "--release", "-j1", "-p",
                "ggfm-android-ffi", "--target", target],
               cwd=root, env=env, check=True)
