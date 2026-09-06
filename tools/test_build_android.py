"""Validate Android compiler selection without starting a compiler."""
import os
from pathlib import Path
import runpy
import unittest
from unittest.mock import patch


class AndroidBuildTests(unittest.TestCase):
    def check_abi(self, abi, target, compiler):
        script = Path(__file__).with_name("build_android.py")
        args = [str(script), "--ndk", "test-ndk"]
        if abi:
            args += ["--abi", abi]
        with patch("sys.argv", args), patch.object(Path, "is_file", return_value=True), \
                patch("subprocess.run") as run:
            runpy.run_path(str(script), run_name="__main__")
        command = run.call_args.args[0]
        env = run.call_args.kwargs["env"]
        self.assertEqual(command[-2:], ["--target", target])
        self.assertIn("-j1", command)
        key = target.replace("-", "_")
        suffix = ".cmd" if os.name == "nt" else ""
        self.assertTrue(env["CC_" + key].endswith(compiler + "28-clang" + suffix))
        self.assertEqual(env["CARGO_TARGET_" + key.upper() + "_LINKER"], env["CC_" + key])
        self.assertEqual(env["CARGO_BUILD_JOBS"], "1")

    def test_default_is_still_arm64(self):
        self.check_abi(None, "aarch64-linux-android", "aarch64-linux-android")

    def test_armv7_uses_ndk_armv7a_compiler(self):
        self.check_abi("armeabi-v7a", "armv7-linux-androideabi", "armv7a-linux-androideabi")


if __name__ == "__main__":
    unittest.main()
