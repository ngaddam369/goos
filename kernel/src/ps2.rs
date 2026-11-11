//! The 8042 PS/2 controller: setting up its second (auxiliary) port for
//! a mouse. The keyboard on the first port needs no setup, the firmware
//! leaves it working.

use core::hint::spin_loop;

use x86_64::instructions::port::Port;

const DATA: u16 = 0x60;
/// Reads give the status register, writes send controller commands.
const STATUS_COMMAND: u16 = 0x64;

/// Status bits.
pub const OUTPUT_FULL: u8 = 1 << 0;
const INPUT_FULL: u8 = 1 << 1;
/// The byte in the output buffer came from the mouse.
pub const FROM_AUX: u8 = 1 << 5;

/// Controller commands.
const READ_CONFIG: u8 = 0x20;
const WRITE_CONFIG: u8 = 0x60;
const DISABLE_AUX: u8 = 0xA7;
const ENABLE_AUX: u8 = 0xA8;
const DISABLE_KEYBOARD: u8 = 0xAD;
const ENABLE_KEYBOARD: u8 = 0xAE;
/// Send the next data byte to the mouse instead of the keyboard.
const TO_AUX: u8 = 0xD4;

/// Controller configuration bits.
const AUX_IRQ_ENABLED: u8 = 1 << 1;
const AUX_CLOCK_DISABLED: u8 = 1 << 5;

/// Mouse commands and its acknowledgement.
const SET_DEFAULTS: u8 = 0xF6;
const ENABLE_REPORTING: u8 = 0xF4;
const ACK: u8 = 0xFA;

/// Status polls before giving up. Each poll is a slow port access (about
/// a microsecond on real hardware, more in a VM), so this is tens of
/// milliseconds at least, longer than the 25 ms a mouse may take to answer.
const TIMEOUT: u32 = 50_000;

pub fn status() -> u8 {
    // SAFETY: reading the 8042 status register has no side effects.
    unsafe { Port::new(STATUS_COMMAND).read() }
}

/// Reads the data port.
///
/// # Safety
/// Consumes the byte in the output buffer: only call when it is known to
/// be full and the byte is meant for the caller.
pub unsafe fn read_data_port() -> u8 {
    // SAFETY: the caller owns the pending byte.
    unsafe { Port::new(DATA).read() }
}

/// Waits until `ready(status)` holds; false on timeout.
fn wait(ready: impl Fn(u8) -> bool) -> bool {
    for _ in 0..TIMEOUT {
        if ready(status()) {
            return true;
        }
        spin_loop();
    }
    false
}

fn send_command(cmd: u8) -> bool {
    // SAFETY: standard 8042 command port; only used during init.
    wait(|s| s & INPUT_FULL == 0) && {
        unsafe { Port::new(STATUS_COMMAND).write(cmd) };
        true
    }
}

fn send_data(byte: u8) -> bool {
    // SAFETY: standard 8042 data port; only used during init.
    wait(|s| s & INPUT_FULL == 0) && {
        unsafe { Port::new(DATA).write(byte) };
        true
    }
}

/// The next byte from the controller itself (the keyboard is disabled
/// while this is used, so nothing else can arrive).
fn read_reply() -> Option<u8> {
    // SAFETY: we're waiting for exactly this byte.
    wait(|s| s & OUTPUT_FULL != 0).then(|| unsafe { read_data_port() })
}

/// Sends a command to the mouse and checks that it acknowledged it.
fn mouse_command(cmd: u8) -> bool {
    if !(send_command(TO_AUX) && send_data(cmd)) {
        return false;
    }
    // Only a byte from the mouse counts as its answer.
    wait(|s| s & OUTPUT_FULL != 0 && s & FROM_AUX != 0)
        // SAFETY: the pending byte is the mouse's reply.
        && unsafe { read_data_port() } == ACK
}

/// Empties the output buffer (bounded, in case the hardware is odd).
fn flush() {
    for _ in 0..16 {
        if status() & OUTPUT_FULL == 0 {
            return;
        }
        // SAFETY: discarding stale bytes during init.
        let _ = unsafe { read_data_port() };
    }
}

fn set_aux_config(enabled: bool) -> bool {
    let Some(config) = send_command(READ_CONFIG).then(read_reply).flatten() else {
        return false;
    };
    let config = if enabled {
        (config | AUX_IRQ_ENABLED) & !AUX_CLOCK_DISABLED
    } else {
        (config & !AUX_IRQ_ENABLED) | AUX_CLOCK_DISABLED
    };
    send_command(WRITE_CONFIG) && send_data(config)
}

/// Enables the mouse port, its interrupt (IRQ12) and data reporting.
/// Call with interrupts disabled. Returns false if there is no mouse, in
/// which case the mouse port is left switched off. The keyboard is
/// re-enabled either way.
pub fn init_mouse() -> bool {
    // No controller at all (reads float high on machines without one).
    if status() == 0xFF {
        return false;
    }
    // Keep keystrokes out of the way while talking to the controller.
    send_command(DISABLE_KEYBOARD);
    send_command(DISABLE_AUX);
    flush();

    let ok = set_aux_config(true)
        && send_command(ENABLE_AUX)
        && mouse_command(SET_DEFAULTS)
        && mouse_command(ENABLE_REPORTING);
    if !ok {
        // Don't leave a half-working mouse filling the output buffer.
        send_command(DISABLE_AUX);
        flush();
        set_aux_config(false);
    }
    flush();
    send_command(ENABLE_KEYBOARD);
    ok
}
