"""Build an isolated ABI test APK; never embeds or modifies a game executable."""
import argparse
import os
from pathlib import Path
import shutil
import subprocess
import zipfile


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--sdk", type=Path, required=True)
    p.add_argument("--java-home", type=Path, required=True)
    p.add_argument("--server", type=Path, required=True)
    p.add_argument("--master", type=Path, required=True)
    p.add_argument("--abi", choices=["armeabi-v7a", "arm64-v8a"], required=True)
    p.add_argument("--output", type=Path, required=True)
    p.add_argument("--test-keystore", type=Path, help="Reuse this harness's test-only android-password key")
    args = p.parse_args()
    root = Path(__file__).resolve().parents[1]
    src = root / "tools/android-smoke"
    sdk = args.sdk.resolve()
    java = args.java_home.resolve() / "bin"
    out = args.output.resolve()
    if out.exists():
        raise SystemExit("output must not exist")
    for source in [args.server, args.master]:
        if not source.is_file():
            raise SystemExit(f"missing input: {source}")
    out.mkdir(parents=True)
    env = dict(os.environ, JAVA_HOME=str(args.java_home.resolve()))
    env["PATH"] = str(java) + os.pathsep + env.get("PATH", "")
    def run(*command):
        subprocess.run(list(map(str, command)), check=True, env=env, cwd=out)
    exe = ".exe" if os.name == "nt" else ""
    bt = sdk / "build-tools/35.0.0"
    platform = sdk / "platforms/android-35/android.jar"
    host = "windows-x86_64" if os.name == "nt" else "linux-x86_64"
    ndk = sdk / "ndk/27.3.13750724/toolchains/llvm/prebuilt" / host / "bin"
    compiler = "armv7a-linux-androideabi" if args.abi == "armeabi-v7a" else "aarch64-linux-android"
    clang = ndk / (compiler + "28-clang" + (".cmd" if os.name == "nt" else ""))
    lib = out / "lib" / args.abi
    lib.mkdir(parents=True)
    shutil.copyfile(args.server, lib / "libggfm_android_ffi.so")
    run(clang, "-shared", "-fPIC", "-Wall", "-Wextra", "-Werror", "-I" + str(root / "include"),
        src / "smoke.c", "-L" + str(lib), "-lggfm_android_ffi", "-landroid",
        "-o", lib / "libggfm_smoke.so")
    classes = out / "classes"
    classes.mkdir()
    run(java / ("javac" + exe), "-encoding", "UTF-8", "-source", "8", "-target", "8",
        "-bootclasspath", platform, "-d", classes, src / "SmokeActivity.java")
    dex = out / "dex"
    dex.mkdir()
    run(java / ("java" + exe), "-cp", bt / "lib/d8.jar", "com.android.tools.r8.D8",
        "--min-api", "28", "--lib", platform, "--output", dex,
        *sorted(classes.rglob("*.class")))
    assets = out / "assets/ggfm"
    assets.mkdir(parents=True)
    shutil.copyfile(args.master, assets / "master.sqlite")
    unsigned = out / "unsigned.apk"
    run(bt / ("aapt" + exe), "package", "-M", src / "AndroidManifest.xml",
        "-I", platform, "-A", out / "assets", "-F", unsigned)
    with zipfile.ZipFile(unsigned, "a", compression=zipfile.ZIP_DEFLATED) as archive:
        archive.write(dex / "classes.dex", "classes.dex")
        for member in sorted(lib.glob("*.so")):
            archive.write(member, f"lib/{args.abi}/{member.name}")
    aligned = out / "aligned.apk"
    run(bt / ("zipalign" + exe), "-p", "4", unsigned, aligned)
    # Ephemeral test-only identity, never the published game's signing key.
    key = args.test_keystore.resolve() if args.test_keystore else out / "smoke-only.jks"
    if args.test_keystore:
        if not key.is_file():
            raise SystemExit("test signing key does not exist")
    else:
        run(java / ("keytool" + exe), "-genkeypair", "-keystore", key,
            "-storepass", "android", "-keypass", "android", "-alias", "smoke",
            "-keyalg", "RSA", "-keysize", "2048", "-validity", "30",
            "-dname", "CN=GGFM isolated ABI smoke")
    apk = out / "ggfm-abi-smoke.apk"
    run(java / ("java" + exe), "-jar", bt / "lib/apksigner.jar", "sign", "--ks", key,
        "--ks-pass", "pass:android", "--out", apk, aligned)
    run(java / ("java" + exe), "-jar", bt / "lib/apksigner.jar", "verify", apk)
    print(apk)


if __name__ == "__main__":
    main()
