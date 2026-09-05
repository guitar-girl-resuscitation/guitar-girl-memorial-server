#!/usr/bin/env python3
"""Build the stable Android ABI with bounded parallelism and an explicit NDK."""
import argparse
import os
from pathlib import Path
import subprocess

p = argparse.ArgumentParser()
p.add_argument("--ndk", type=Path, required=True)
args = p.parse_args()
root = Path(__file__).resolve().parents[1]
host = "windows-x86_64" if os.name == "nt" else "linux-x86_64"
bin_dir = args.ndk.resolve() / "toolchains/llvm/prebuilt" / host / "bin"
cc = bin_dir / ("aarch64-linux-android28-clang.cmd" if os.name == "nt" else "aarch64-linux-android28-clang")
ar = bin_dir / ("llvm-ar.exe" if os.name == "nt" else "llvm-ar")
if not cc.is_file() or not ar.is_file():
    raise SystemExit("NDK compiler missing")
env = dict(os.environ)
env.update(CC_aarch64_linux_android=str(cc), AR_aarch64_linux_android=str(ar),
           CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER=str(cc), CARGO_BUILD_JOBS="2")
subprocess.run(["cargo", "build", "--locked", "--release", "-j2", "-p",
                "ggfm-android-ffi", "--target", "aarch64-linux-android"],
               cwd=root, env=env, check=True)
