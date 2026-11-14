//! Host-side launcher: boots the GoOS disk image in QEMU.
//!
//! Usage: `cargo run --release [-- [bios|uefi] [load FILE.sgf] | images]`
//! (BIOS is the default; `images` just prints where the disk images are).
//!
//! The kernel's serial output is shown in the terminal. Game records it
//! sends (between `-----BEGIN SGF-----` and `-----END SGF-----`) are also
//! saved to `goos-<time>.sgf` in the current directory, and `load FILE`
//! sends a game record to the kernel once it has booted.
//!
//! Environment variables:
//! - `GOOS_AUDIO=pipewire|pa|none`  force an audio backend for the PC speaker
//! - `GOOS_DEBUG=1`                 software emulation + interrupt log in qemu.log
//! - `GOOS_HEADLESS=1`              no window; QEMU monitor on a unix socket
//! - `GOOS_MONITOR=<path>`          monitor socket path (default target/goos-monitor.sock)
//! - `GOOS_OVMF=<path>`             UEFI firmware image for `uefi` (default: search known paths)

use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;
use std::process::{ChildStdin, ChildStdout, Command, Stdio, exit};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

const QEMU: &str = "qemu-system-x86_64";
/// Where distributions install the combined OVMF (UEFI) firmware image.
const OVMF_PATHS: &[&str] = &[
    "/usr/share/edk2/x64/OVMF.4m.fd", // Arch (edk2-ovmf)
    "/usr/share/ovmf/OVMF.fd",        // Debian/Ubuntu (ovmf)
    "/usr/share/OVMF/OVMF.fd",
    "/usr/share/edk2/ovmf/OVMF.fd",    // some Fedora versions
    "/usr/share/qemu/ovmf-x86_64.bin", // openSUSE
];

/// Lines the kernel prints around game records, and when it's ready for input.
const SGF_BEGIN: &str = "-----BEGIN SGF-----";
const SGF_END: &str = "-----END SGF-----";
const READY: &str = "GoOS ready";

fn main() {
    let mut uefi = false;
    let mut load = None;
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "bios" => uefi = false,
            "uefi" => uefi = true,
            "images" => {
                println!("BIOS image: {}", env!("BIOS_PATH"));
                println!("UEFI image: {}", env!("UEFI_PATH"));
                return;
            }
            "load" => {
                let Some(path) = args.next() else {
                    eprintln!("`load` needs a file name");
                    exit(2);
                };
                match fs::read(&path) {
                    Ok(data) => load = Some(data),
                    Err(e) => {
                        eprintln!("can't read {path}: {e}");
                        exit(1);
                    }
                }
            }
            other => {
                eprintln!(
                    "unknown argument {other:?}; expected `bios`, `uefi`, `load FILE` or `images`"
                );
                exit(2);
            }
        }
    }

    if Command::new(QEMU).arg("--version").output().is_err() {
        eprintln!("{QEMU} not found; install QEMU (see README.md)");
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
        let firmware = env::var("GOOS_OVMF").ok().or_else(|| {
            OVMF_PATHS
                .iter()
                .find(|p| Path::new(p).exists())
                .map(|p| p.to_string())
        });
        let Some(firmware) = firmware else {
            eprintln!("no UEFI firmware found; install OVMF or set GOOS_OVMF to OVMF.fd");
            exit(1);
        };
        cmd.arg("-bios").arg(firmware);
        cmd.arg("-drive")
            .arg(format!("format=raw,file={}", env!("UEFI_PATH")));
    } else {
        cmd.arg("-drive")
            .arg(format!("format=raw,file={}", env!("BIOS_PATH")));
    }

    if debug {
        cmd.args([
            "-accel",
            "tcg",
            "-d",
            "int,cpu_reset",
            "-D",
            "qemu.log",
            "-no-shutdown",
        ]);
    } else if kvm_available() {
        cmd.args(["-accel", "kvm", "-cpu", "host"]);
    }

    if headless {
        let monitor = env::var("GOOS_MONITOR").unwrap_or_else(|_| {
            concat!(env!("CARGO_MANIFEST_DIR"), "/../target/goos-monitor.sock").into()
        });
        cmd.args(["-display", "none"]);
        cmd.arg("-monitor")
            .arg(format!("unix:{monitor},server,nowait"));
    } else {
        cmd.args(["-display", "gtk,zoom-to-fit=on"]);
    }

    // Serial output comes through us, so game records can be saved; when
    // loading, so does serial input.
    cmd.stdout(Stdio::piped());
    if load.is_some() {
        cmd.stdin(Stdio::piped());
    }

    eprintln!(
        "launching QEMU ({} boot, audio: {audio})",
        if uefi { "UEFI" } else { "BIOS" }
    );
    let mut child = cmd.spawn().expect("failed to run QEMU");
    let stdout = child.stdout.take().expect("piped stdout");
    let stdin = child.stdin.take();
    let watcher = thread::spawn(move || watch_serial(stdout, stdin, load));
    let status = child.wait().expect("failed to wait for QEMU");
    let _ = watcher.join();
    exit(status.code().unwrap_or(1));
}

/// Echoes the kernel's serial output to the terminal, saves the game
/// records in it to files, and sends `load` once the kernel is ready.
fn watch_serial(stdout: ChildStdout, mut stdin: Option<ChildStdin>, mut load: Option<Vec<u8>>) {
    let mut reader = BufReader::new(stdout);
    let mut terminal = io::stdout();
    let mut line = Vec::new();
    let mut record: Option<String> = None;
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let _ = terminal.write_all(&line);
        let _ = terminal.flush();

        let text = String::from_utf8_lossy(&line);
        let text = text.trim_end();
        if text == READY {
            if let (Some(data), Some(pipe)) = (load.take(), stdin.as_mut()) {
                let sent = pipe.write_all(&data).and_then(|()| pipe.flush());
                match sent {
                    Ok(()) => eprintln!("[runner] sent the game record ({} bytes)", data.len()),
                    Err(e) => eprintln!("[runner] couldn't send the game record: {e}"),
                }
            }
        } else if text == SGF_BEGIN {
            record = Some(String::new());
        } else if text == SGF_END {
            if let Some(sgf) = record.take() {
                save_record(&sgf);
            }
        } else if let Some(sgf) = record.as_mut() {
            sgf.push_str(text);
            sgf.push('\n');
        }
    }
}

/// Writes a record to a new file `goos-<seconds>[-n].sgf`, never
/// overwriting an existing one.
fn save_record(sgf: &str) {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    for n in 0..100 {
        let path = if n == 0 {
            format!("goos-{secs}.sgf")
        } else {
            format!("goos-{secs}-{n}.sgf")
        };
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                match file.write_all(sgf.as_bytes()) {
                    Ok(()) => eprintln!("[runner] saved the game record to {path}"),
                    Err(e) => eprintln!("[runner] couldn't save the game record: {e}"),
                }
                return;
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                eprintln!("[runner] couldn't save the game record: {e}");
                return;
            }
        }
    }
    eprintln!("[runner] couldn't find a free file name for the game record");
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
    OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/kvm")
        .is_ok()
}
