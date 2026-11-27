#!/usr/bin/env python3
"""A development VM for the Linux host: real cold boots to Plasma Login, root without asking,
and crashes that cost a VM reset, for the work around sign-in (the system service, KMS capture
of the login screen, session handover). It has no AMD GPU: no VA-API encoding, which stays a
check on real hardware.

  vm.py create              download Arch's cloud image (SHA-256 and signature checked), make
                            the disk, the SSH key and cloud-init's first-boot config
  vm.py start [--seed]      boot; --seed on the first boot (cloud-init makes the user)
  vm.py provision           install Plasma, Plasma Login and the host's dependencies, enable
                            the login manager (no autologin: every boot ends at the greeter)
  vm.py stop | kill | reset | status
  vm.py ssh [command...]    as `dev` (password `rubylight`, passwordless sudo)
  vm.py push SOURCE... DEST copy files into the VM (DEST relative to /home/dev)
  vm.py shot FILE.png       screenshot of the VM's screen
  vm.py key ctrl-alt-f2     press key combinations (QEMU key names)
  vm.py type TEXT           type text (US layout)
  vm.py click X Y [right|middle]   click at a pixel of the current screen
  vm.py snap save|load|list|delete [NAME]   disk snapshots, with the VM off

The VM lives in $RUBYLIGHT_VM_DIR (default ~/VMs/rubylight-dev): KVM, 6 vCPUs, 8 GB, UEFI, a
2D virtio-gpu at 1920x1080 (the guest renders in software; QEMU cannot screenshot a virgl
scanout), an HDA sound card, user networking with SSH on 127.0.0.1:2222 and the host's ports
on 127.0.0.1 at +10000 (web console 57990). Watch it with a VNC viewer on 127.0.0.1:5905.
Needs qemu-desktop, edk2-ovmf, gpg and access to /dev/kvm.
"""
import hashlib
import json
import os
import shutil
import socket
import struct
import subprocess
import sys
import time
import urllib.request

HERE = os.path.expanduser(os.environ.get("RUBYLIGHT_VM_DIR", "~/VMs/rubylight-dev"))
IMAGES = os.path.join(HERE, "images")
QMP = os.path.join(HERE, "qmp.sock")
PID = os.path.join(HERE, "qemu.pid")
KEY = os.path.join(HERE, "ssh", "id_ed25519")
OVMF = "/usr/share/edk2/x64"
MIRROR = "https://geo.mirror.pkgbuild.com/images/latest"
IMAGE = "Arch-Linux-x86_64-cloudimg.qcow2"
# arch-boxes <arch-boxes@archlinux.org>, the key Arch's VM images are signed with.
SIGNER = "1B9A16984A4E8CB448712D2AE0B78BF4326C6F8F"
SEED_PORT = 8642
SSH_PORT = 2222
# The host's ports, forwarded at +10000 so they do not meet the PC's own host.
TCP = [47984, 47989, 47990, 48010]
UDP = [47998, 47999, 48000, 48002, 48010]
PACKAGES = (
    "plasma-desktop plasma-login-manager kscreen plasma-pa konsole xdg-desktop-portal-kde "
    "breeze noto-fonts pipewire pipewire-pulse pipewire-alsa wireplumber libva libdrm mesa "
    "vulkan-swrast vulkan-virtio vulkan-tools mesa-utils libva-utils gamescope xorg-xwayland "
    "wl-clipboard xdg-utils ffmpeg at-spi2-core zenity rsync gdb strace"
)


def running():
    try:
        pid = int(open(PID).read())
        os.kill(pid, 0)
        return pid
    except (OSError, ValueError):
        return None


def fetch(url, path):
    with urllib.request.urlopen(url) as response, open(path, "wb") as out:
        shutil.copyfileobj(response, out)


def create():
    for folder in ("images", "ssh", "seed", "download"):
        os.makedirs(os.path.join(HERE, folder), exist_ok=True)
    base = os.path.join(IMAGES, "base.qcow2")
    if not os.path.exists(base):
        download = os.path.join(HERE, "download")
        image = os.path.join(download, IMAGE)
        for name in (IMAGE, IMAGE + ".SHA256", IMAGE + ".sig"):
            print("downloading", name)
            fetch(f"{MIRROR}/{name}", os.path.join(download, name))
        expected = open(image + ".SHA256").read().split()[0]
        digest = hashlib.sha256()
        with open(image, "rb") as f:
            for block in iter(lambda: f.read(1 << 20), b""):
                digest.update(block)
        if digest.hexdigest() != expected:
            raise SystemExit("the image's SHA-256 does not match")
        home = os.path.join(HERE, "gnupg")
        os.makedirs(home, mode=0o700, exist_ok=True)
        gpg = ["gpg", "--homedir", home, "--batch"]
        subprocess.run(gpg + ["--keyserver", "hkps://keyserver.ubuntu.com", "--recv-keys",
                              SIGNER], check=True)
        status = subprocess.run(gpg + ["--status-fd", "1", "--verify", image + ".sig", image],
                                capture_output=True, text=True).stdout
        if "VALIDSIG" not in status or SIGNER not in status:
            raise SystemExit("the image is not signed by arch-boxes")
        os.replace(image, base)
    vars_ = os.path.join(IMAGES, "OVMF_VARS.4m.fd")
    if not os.path.exists(vars_):
        shutil.copy(f"{OVMF}/OVMF_VARS.4m.fd", vars_)
    disk = os.path.join(IMAGES, "disk.qcow2")
    if not os.path.exists(disk):
        subprocess.run(["qemu-img", "create", "-q", "-f", "qcow2", "-b", "base.qcow2", "-F",
                        "qcow2", disk, "80G"], check=True)
    if not os.path.exists(KEY):
        subprocess.run(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-C", "rubylight-dev",
                        "-f", KEY], check=True)
    public = open(KEY + ".pub").read().strip()
    seed = os.path.join(HERE, "seed")
    open(os.path.join(seed, "meta-data"), "w").write(
        "instance-id: rubylight-dev-1\nlocal-hostname: rubylight-vm\n")
    open(os.path.join(seed, "vendor-data"), "w").write("")
    open(os.path.join(seed, "user-data"), "w").write(f"""#cloud-config
hostname: rubylight-vm
users:
  - name: dev
    gecos: Rubylight Dev
    groups: [wheel, video, input, audio]
    sudo: ALL=(ALL) NOPASSWD:ALL
    shell: /bin/bash
    lock_passwd: false
    ssh_authorized_keys:
      - {public}
chpasswd:
  expire: false
  users:
    - {{name: dev, password: rubylight, type: text}}
ssh_pwauth: false
timezone: Europe/Paris
""")
    print(f"made in {HERE}; next: vm.py start --seed, then vm.py provision")


class Qmp:
    def __init__(self):
        self.sock = socket.socket(socket.AF_UNIX)
        self.sock.connect(QMP)
        self.file = self.sock.makefile("rw")
        self.file.readline()
        self.run("qmp_capabilities")

    def run(self, command, **arguments):
        message = {"execute": command}
        if arguments:
            message["arguments"] = arguments
        self.file.write(json.dumps(message) + "\n")
        self.file.flush()
        while True:
            reply = json.loads(self.file.readline())
            if "return" in reply:
                return reply["return"]
            if "error" in reply:
                raise SystemExit(f"{command}: {reply['error']['desc']}")


def start(seed):
    if running():
        print("already running")
        return
    if seed:
        # Served only to the VM's first boot, from the host's loopback (10.0.2.2 in the VM).
        subprocess.Popen(
            [sys.executable, "-I", "-m", "http.server", str(SEED_PORT), "--bind", "127.0.0.1",
             "--directory", os.path.join(HERE, "seed")],
            stdout=open(os.path.join(HERE, "seed.log"), "w"), stderr=subprocess.STDOUT,
            start_new_session=True,
        )
    forwards = [f"hostfwd=tcp:127.0.0.1:{SSH_PORT}-:22"]
    forwards += [f"hostfwd=tcp:127.0.0.1:{p + 10000}-:{p}" for p in TCP]
    forwards += [f"hostfwd=udp:127.0.0.1:{p + 10000}-:{p}" for p in UDP]
    command = [
        "qemu-system-x86_64", "-name", "rubylight-dev", "-enable-kvm",
        "-machine", "q35", "-cpu", "host", "-smp", "6", "-m", "8G",
        "-drive", f"if=pflash,format=raw,readonly=on,file={OVMF}/OVMF_CODE.4m.fd",
        "-drive", f"if=pflash,format=raw,file={IMAGES}/OVMF_VARS.4m.fd",
        "-drive", f"file={IMAGES}/disk.qcow2,if=virtio,discard=unmap",
        "-device", "virtio-vga,id=gpu,xres=1920,yres=1080", "-display", "none",
        "-vnc", "127.0.0.1:5",
        "-device", "virtio-tablet-pci", "-device", "virtio-keyboard-pci",
        "-audiodev", "none,id=snd", "-device", "intel-hda", "-device", "hda-duplex,audiodev=snd",
        "-device", "virtio-rng-pci",
        "-nic", "user,model=virtio-net-pci," + ",".join(forwards),
        "-qmp", f"unix:{QMP},server=on,wait=off",
        "-serial", f"file:{HERE}/serial.log",
        "-daemonize", "-pidfile", PID,
    ]
    if seed:
        command += ["-smbios", f"type=1,serial=ds=nocloud;s=http://10.0.2.2:{SEED_PORT}/"]
    subprocess.run(command, check=True)
    print("started; serial console in", os.path.join(HERE, "serial.log"))


def ssh_command(rest):
    return ["ssh", "-i", KEY, "-p", str(SSH_PORT), "-o", "StrictHostKeyChecking=accept-new",
            "-o", f"UserKnownHostsFile={HERE}/ssh/known_hosts", "-o", "ConnectTimeout=5",
            "-o", "LogLevel=ERROR", "dev@127.0.0.1"] + rest


def provision():
    script = (
        f"sudo pacman -Syu --noconfirm --needed {PACKAGES} && "
        "sudo systemctl enable plasmalogin.service && "
        "sudo systemctl set-default graphical.target"
    )
    subprocess.run(ssh_command([script]), check=True)
    print("provisioned; restart the VM (vm.py stop, vm.py start) to reach the greeter")


def push(sources, dest):
    ssh = " ".join(ssh_command([])[:-1])
    subprocess.run(["rsync", "-a", "-e", ssh] + sources + [f"dev@127.0.0.1:{dest}"],
                   check=True)


def wait_gone(seconds):
    end = time.time() + seconds
    while running() and time.time() < end:
        time.sleep(0.5)
    return not running()


def screen_size(qmp):
    path = os.path.join(HERE, ".size.png")
    qmp.run("screendump", filename=path, format="png")
    with open(path, "rb") as f:
        head = f.read(24)
    os.remove(path)
    return struct.unpack(">II", head[16:24])


KEYS = {" ": "spc", "\n": "ret", "\t": "tab", "-": "minus", "=": "equal", ".": "dot",
        ",": "comma", "/": "slash", ";": "semicolon", "'": "apostrophe", "[": "bracket_left",
        "]": "bracket_right", "\\": "backslash", "`": "grave_accent"}
SHIFTED = {"_": "-", "+": "=", ">": ".", "<": ",", "?": "/", ":": ";", '"': "'", "{": "[",
           "}": "]", "|": "\\", "~": "`", "!": "1", "@": "2", "#": "3", "$": "4", "%": "5",
           "^": "6", "&": "7", "*": "8", "(": "9", ")": "0"}


def press(qmp, names):
    keys = [{"type": "qcode", "data": n} for n in names]
    qmp.run("send-key", keys=keys, **{"hold-time": 60})
    time.sleep(0.04)


def type_text(qmp, text):
    for ch in text:
        if ch.isupper():
            press(qmp, ["shift", ch.lower()])
        elif ch in SHIFTED:
            base = SHIFTED[ch]
            press(qmp, ["shift", KEYS.get(base, base)])
        else:
            press(qmp, [KEYS.get(ch, ch)])


def click(qmp, x, y, button):
    width, height = screen_size(qmp)
    events = [
        {"type": "abs", "data": {"axis": "x", "value": int(x * 32767 / max(width - 1, 1))}},
        {"type": "abs", "data": {"axis": "y", "value": int(y * 32767 / max(height - 1, 1))}},
    ]
    qmp.run("input-send-event", events=events)
    time.sleep(0.05)
    for down in (True, False):
        qmp.run("input-send-event",
                events=[{"type": "btn", "data": {"down": down, "button": button}}])
        time.sleep(0.05)


def snap(action, name):
    if running() and action in ("save", "load", "delete"):
        raise SystemExit("stop the VM first")
    disk = os.path.join(IMAGES, "disk.qcow2")
    vars_ = os.path.join(IMAGES, "OVMF_VARS.4m.fd")
    flag = {"save": "-c", "load": "-a", "delete": "-d", "list": "-l"}[action]
    subprocess.run(["qemu-img", "snapshot", flag] + ([name] if name else []) + [disk],
                   check=True)
    keep = f"{vars_}.{name}"
    if action == "save":
        shutil.copy(vars_, keep)
    elif action == "load" and os.path.exists(keep):
        shutil.copy(keep, vars_)


def main():
    args = sys.argv[1:]
    if not args:
        raise SystemExit(__doc__)
    command, rest = args[0], args[1:]
    if command == "create":
        create()
    elif command == "start":
        start("--seed" in rest)
    elif command == "provision":
        provision()
    elif command == "status":
        print(f"running, pid {running()}" if running() else "stopped")
    elif command == "ssh":
        os.execvp("ssh", ssh_command(rest))
    elif command == "push":
        push(rest[:-1], rest[-1])
    elif command == "snap":
        snap(rest[0], rest[1] if len(rest) > 1 else None)
    else:
        if not running():
            raise SystemExit("the VM is not running")
        qmp = Qmp()
        if command == "stop":
            # A desktop session answers the power button with a dialog: power off from inside,
            # and press the button only when SSH does not answer.
            inside = subprocess.run(ssh_command(["sudo systemctl poweroff"]),
                                    capture_output=True, timeout=30)
            if inside.returncode not in (0, 255) or not wait_gone(10):
                qmp.run("system_powerdown")
            print("stopped" if wait_gone(90) else "still running after 90 s (try kill)")
        elif command == "kill":
            qmp.run("quit")
        elif command == "reset":
            qmp.run("system_reset")
        elif command == "shot":
            qmp.run("screendump", filename=os.path.abspath(rest[0]), format="png")
        elif command == "key":
            for combo in rest:
                press(qmp, combo.split("-") if combo != "minus" else ["minus"])
        elif command == "type":
            type_text(qmp, " ".join(rest))
        elif command == "click":
            click(qmp, int(rest[0]), int(rest[1]), rest[2] if len(rest) > 2 else "left")
        else:
            raise SystemExit(__doc__)


if __name__ == "__main__":
    main()
