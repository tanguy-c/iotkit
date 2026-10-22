#!/usr/bin/env python3
"""Hardware-in-the-loop test on an ESP32-C6 DevKit: flash devkit over USB, wait until healthy,
push a newer version over OTA and check it is kept, push one that never gets healthy and one that
crashes before `iotkit::start`, and check the device rolls back by itself each time.

Usage (from the repository root): tools/hil/run.py [--port /dev/ttyACM0] [--skip-flash]
Needs: espflash, pyserial, the device's Wi-Fi and broker given at build time (WIFI_SSID,
WIFI_PASS, MQTT_HOST in the environment or .cargo/secrets.toml), this machine on that network.

The serial port is opened without touching DTR/RTS: toggling them resets the C6.
"""

import argparse
import glob
import os
import re
import shutil
import subprocess
import sys
import time

import serial

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
WORK = os.path.join(ROOT, "target", "hil")
ANSI = re.compile(rb"\x1b\[[0-9;]*m")


def step(text):
    print(f"\n== {text}", flush=True)


def run(cmd, cwd=ROOT, env=None, check=True):
    print("$ " + " ".join(cmd), flush=True)
    result = subprocess.run(cmd, cwd=cwd, env={**os.environ, **(env or {})}, text=True, capture_output=True)
    if check and result.returncode != 0:
        sys.exit(f"FAIL: {' '.join(cmd)}\n{result.stdout[-2000:]}{result.stderr[-2000:]}")
    return result


def wait_port(timeout=60):
    """The port, once it is there (it goes away while the chip resets)."""
    deadline = time.time() + timeout
    while time.time() < deadline:
        ports = sorted(glob.glob("/dev/ttyACM*"))
        if ports:
            return ports[0]
        time.sleep(0.2)
    sys.exit("FAIL: no /dev/ttyACM* port")


class Console:
    def __init__(self, port):
        self.port = port
        self.serial = serial.Serial(port, 115200, timeout=0.2)
        self.seen = b""

    def close(self):
        self.serial.close()

    def wait_for(self, pattern, timeout):
        """Reads the log until `pattern` (a regex) appears; returns the match or None."""
        deadline = time.time() + timeout
        while time.time() < deadline:
            try:
                self.seen += ANSI.sub(b"", self.serial.read(4096))
            except serial.SerialException:  # the chip restarted: the port comes back
                self.serial.close()
                time.sleep(0.5)
                self.serial = serial.Serial(wait_port(), 115200, timeout=0.2)
                continue
            match = re.search(pattern.encode(), self.seen)
            if match:
                self.seen = self.seen[match.end():]
                return match
        return None

    def command(self, line, expect, timeout=5):
        self.seen = b""
        self.serial.write(line.encode() + b"\r\n")
        return self.wait_for(expect, timeout)


def version_of(cargo_toml):
    return re.search(r'^version\s*=\s*"([^"]+)"', open(cargo_toml).read(), re.M).group(1)


def next_patch(version, n):
    major, minor, patch = (int(p) for p in version.split("."))
    return f"{major}.{minor}.{patch + n}"


def tree_with_version(name, version):
    """A worktree of HEAD under target/hil/ with devkit at `version`, the same signing key."""
    path = os.path.join(WORK, name)
    run(["git", "worktree", "remove", "-f", path], check=False)
    shutil.rmtree(path, ignore_errors=True)
    run(["git", "worktree", "prune"])
    run(["git", "worktree", "add", "-q", "--detach", path, "HEAD"])
    shutil.copytree(os.path.join(ROOT, "keys"), os.path.join(path, "keys"), dirs_exist_ok=True)
    secrets = os.path.join(ROOT, ".cargo", "secrets.toml")
    if os.path.exists(secrets):
        shutil.copy(secrets, os.path.join(path, ".cargo", "secrets.toml"))
    cargo_toml = os.path.join(path, "apps", "devkit", "Cargo.toml")
    text = re.sub(r'^version(\s*)=(\s*)"[^"]+"', f'version\\1=\\2"{version}"', open(cargo_toml).read(), count=1, flags=re.M)
    open(cargo_toml, "w").write(text)
    return path


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--port", help="default: the first /dev/ttyACM*")
    parser.add_argument("--skip-flash", action="store_true", help="test the firmware already on the device")
    args = parser.parse_args()
    os.makedirs(WORK, exist_ok=True)
    if not os.path.exists(os.path.join(ROOT, "keys", "ota.key")):
        run(["cargo", "xtask", "keygen"])
    base = version_of(os.path.join(ROOT, "apps", "devkit", "Cargo.toml"))
    env = {"CARGO_TARGET_DIR": os.path.join(ROOT, "target")}

    if not args.skip_flash:
        step(f"flash devkit {base} over USB")
        run(["cargo", "build", "--release", "-p", "devkit"])
        port = args.port or wait_port()
        run(["espflash", "flash", "--chip", "esp32c6", "--flash-size", "8mb", "--partition-table", "partitions.csv",
             "--erase-parts", "otadata", "--port", port, "target/riscv32imac-unknown-none-elf/release/devkit"])

    step("boots and gets healthy")
    console = Console(args.port or wait_port())
    if not args.skip_flash and not console.wait_for(r"healthy", 90):
        sys.exit("FAIL: not healthy within 90 s:\n" + console.seen.decode(errors="replace")[-3000:])
    info = console.command("info", r"devkit (\S+), up")
    if not info or info.group(1).decode() != base:
        sys.exit(f"FAIL: `info` does not report devkit {base}")
    net = console.command("net", r"(\S+\.local) at (\d+\.\d+\.\d+\.\d+)")
    if not net:
        sys.exit("FAIL: no address (`net`)")
    host = net.group(2).decode()
    console.close()
    print(f"ok: devkit {base} healthy at {host}")

    good = next_patch(base, 1)
    step(f"OTA push {good}: installed, healthy, kept")
    tree = tree_with_version("good", good)
    run(["cargo", "run", "--quiet", "--manifest-path", "xtask/Cargo.toml", "--target", "x86_64-unknown-linux-gnu",
         "--bin", "cargo-iotkit", "--", "ota", "push", "--host", host, "--release"], cwd=tree, env=env)
    console = Console(wait_port())
    if not console.wait_for(r"healthy", 120):
        sys.exit("FAIL: the update did not get healthy:\n" + console.seen.decode(errors="replace")[-3000:])
    if not console.command("info", rf"last update: {re.escape(good)} Confirmed"):
        sys.exit(f"FAIL: `info` does not report {good} confirmed")
    console.close()
    print(f"ok: {good} installed and confirmed")

    bad = next_patch(base, 2)
    step(f"OTA push {bad}, which panics at boot: rolled back to {good}")
    tree = tree_with_version("bad", bad)
    pushed = run(["cargo", "run", "--quiet", "--manifest-path", "xtask/Cargo.toml", "--target", "x86_64-unknown-linux-gnu",
                  "--bin", "cargo-iotkit", "--", "ota", "push", "--host", host, "--release"],
                 cwd=tree, env={**env, "DEVKIT_PANIC_AT_BOOT": "1"}, check=False)
    if "rolled back" not in pushed.stdout + pushed.stderr:
        sys.exit("FAIL: push did not report a rollback:\n" + pushed.stdout[-2000:] + pushed.stderr[-2000:])
    console = Console(wait_port())
    info = console.command("info", r"devkit (\S+), up[\s\S]*?last update: (\S+) (\S+)", timeout=10)
    if not info or info.group(1).decode() != good or info.group(2).decode() != bad:
        sys.exit("FAIL: after the rollback `info` should report devkit "
                 f"{good} and the failed update {bad}:\n" + console.seen.decode(errors="replace")[-2000:])
    console.close()
    print(f"ok: {bad} rolled back to {good} ({info.group(3).decode()})")

    early = next_patch(base, 3)
    step(f"OTA push {early}, which panics before `iotkit::start`: the early guard rolls back to {good}")
    tree = tree_with_version("early", early)
    pushed = run(["cargo", "run", "--quiet", "--manifest-path", "xtask/Cargo.toml", "--target", "x86_64-unknown-linux-gnu",
                  "--bin", "cargo-iotkit", "--", "ota", "push", "--host", host, "--release"],
                 cwd=tree, env={**env, "DEVKIT_PANIC_EARLY": "1"}, check=False)
    if "rolled back" not in pushed.stdout + pushed.stderr:
        sys.exit("FAIL: push did not report a rollback:\n" + pushed.stdout[-2000:] + pushed.stderr[-2000:])
    console = Console(wait_port())
    info = console.command("info", r"devkit (\S+), up[\s\S]*?last update: (\S+) (\S+)", timeout=10)
    if not info or info.group(1).decode() != good or info.group(2).decode() != early:
        sys.exit("FAIL: after the early rollback `info` should report devkit "
                 f"{good} and the failed update {early}:\n" + console.seen.decode(errors="replace")[-2000:])
    console.close()
    print(f"ok: {early} rolled back to {good} by the early guard")

    print("\nPASS")


if __name__ == "__main__":
    main()
