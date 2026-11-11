//! Host-side launcher: boots the GoOS disk image in QEMU.
//!
//! Usage: `cargo run --release [-- bios|uefi]` (BIOS is the default).
//!
//! Environment variables:
//! - `GOOS_AUDIO=pipewire|pa|none`  force an audio backend for the PC speaker
//! - `GOOS_DEBUG=1`                 software emulation + interrupt log in qemu.log
//! - `GOOS_HEADLESS=1`              no window; QEMU monitor on a unix socket
//! - `GOOS_MONITOR=<path>`          monitor socket path (default target/goos-monitor.sock)

use std::env;
use std::fs::OpenOptions;
use std::path::Path;
use std::process::{Command, exit};

const QEMU: &str = "qemu-system-x86_64";
const OVMF: &str = "/usr/share/edk2/x64/OVMF.4m.fd";

fn main() {
    let uefi = match env::args().nth(1).as_deref() {
        None | Some("bios") => false,
        Some("uefi") => true,
        Some(other) => {
            eprintln!("unknown boot mode {other:?}; expected `bios` or `uefi`");
            exit(2);
        }
    };

    if Command::new(QEMU).arg("--version").output().is_err() {
        eprintln!("{QEMU} not found. On Arch: sudo pacman -S qemu-system-x86 qemu-ui-gtk qemu-audio-pipewire");
        exit(1);
    }

    let flag = |name: &str| env::var(name).is_ok_and(|v| v == "1");
    let debug = flag("GOOS_DEBUG");
    let headless = flag("GOOS_HEADLESS");

    let audio = audio_backend();
    let mut cmd = Command::new(QEMU);
    cmd.args(["-machine", "pc,pcspk-audiodev=snd0"]);
    cmd.arg("-audiodev").arg(format!("{audio},id=snd0"));
    cmd.args(["-m", "256M", "-serial", "stdio", "-no-reboot"]);

    if uefi {
        if !Path::new(OVMF).exists() {
            eprintln!("UEFI firmware {OVMF} not found (install edk2-ovmf)");
            exit(1);
        }
        cmd.args(["-bios", OVMF]);
        cmd.arg("-drive").arg(format!("format=raw,file={}", env!("UEFI_PATH")));
    } else {
        cmd.arg("-drive").arg(format!("format=raw,file={}", env!("BIOS_PATH")));
    }

    if debug {
        cmd.args(["-accel", "tcg", "-d", "int,cpu_reset", "-D", "qemu.log", "-no-shutdown"]);
    } else if kvm_available() {
        cmd.args(["-accel", "kvm", "-cpu", "host"]);
    }

    if headless {
        let monitor = env::var("GOOS_MONITOR")
            .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../target/goos-monitor.sock").into());
        cmd.args(["-display", "none"]);
        cmd.arg("-monitor").arg(format!("unix:{monitor},server,nowait"));
    } else {
        cmd.args(["-display", "gtk,zoom-to-fit=on"]);
    }

    eprintln!("launching QEMU ({} boot, audio: {audio})", if uefi { "UEFI" } else { "BIOS" });
    let status = cmd.status().expect("failed to run QEMU");
    exit(status.code().unwrap_or(1));
}

/// Picks an audio backend for the PC speaker. QEMU refuses to start if an
/// explicitly requested backend can't be opened, so only pick a real one
/// when both the QEMU driver and the sound server are present.
fn audio_backend() -> String {
    if let Ok(forced) = env::var("GOOS_AUDIO") {
        return forced;
    }
    let drivers = Command::new(QEMU)
        .args(["-audiodev", "help"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    let has_driver = |name: &str| drivers.lines().any(|l| l.trim() == name);
    let runtime = env::var("XDG_RUNTIME_DIR").unwrap_or_default();

    if has_driver("pipewire") && Path::new(&runtime).join("pipewire-0").exists() {
        "pipewire".into()
    } else if has_driver("pa") && Path::new(&runtime).join("pulse/native").exists() {
        "pa".into()
    } else {
        "none".into()
    }
}

fn kvm_available() -> bool {
    OpenOptions::new().read(true).write(true).open("/dev/kvm").is_ok()
}
