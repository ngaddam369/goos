//! Programmable Interval Timer (8253/8254): channel 0 drives the timer
//! interrupt, channel 2 drives the PC speaker.

use x86_64::instructions::interrupts::without_interrupts;
use x86_64::instructions::port::Port;

/// Input clock of the PIT in Hz.
const PIT_HZ: u32 = 1_193_182;

const CHANNEL0: u16 = 0x40;
const CHANNEL2: u16 = 0x42;
const COMMAND: u16 = 0x43;
/// Keyboard controller port B: bit 0 gates channel 2, bit 1 enables the speaker.
const SPEAKER_GATE: u16 = 0x61;

/// The 16-bit reload value for a square wave of `hz` (clamped to what the
/// PIT can produce, roughly 19 Hz to 1.19 MHz).
fn divisor(hz: u32) -> u16 {
    (PIT_HZ / hz.max(1)).clamp(1, u16::MAX as u32) as u16
}

/// Writes a mode byte and a divisor to one PIT channel.
fn program(channel_port: u16, mode: u8, divisor: u16) {
    // SAFETY: standard PIT ports. Interrupts are off so the two-byte write
    // sequence can't be interleaved with another PIT access.
    without_interrupts(|| unsafe {
        Port::<u8>::new(COMMAND).write(mode);
        let mut data = Port::<u8>::new(channel_port);
        data.write(divisor as u8);
        data.write((divisor >> 8) as u8);
    });
}

/// Makes channel 0 (wired to IRQ0) fire `hz` times per second.
pub fn set_timer_hz(hz: u32) {
    // Channel 0, low byte then high byte, mode 3 (square wave), binary.
    program(CHANNEL0, 0x36, divisor(hz));
}

/// Starts a tone of `hz` on the PC speaker until `speaker_off` is called.
pub fn speaker_on(hz: u32) {
    // Channel 2, low byte then high byte, mode 3 (square wave), binary.
    program(CHANNEL2, 0xB6, divisor(hz));
    // SAFETY: read-modify-write of port 0x61, only touching the speaker bits.
    without_interrupts(|| unsafe {
        let mut gate = Port::<u8>::new(SPEAKER_GATE);
        let v = gate.read();
        gate.write(v | 0b11);
    });
}

pub fn speaker_off() {
    // SAFETY: as in `speaker_on`.
    without_interrupts(|| unsafe {
        let mut gate = Port::<u8>::new(SPEAKER_GATE);
        let v = gate.read();
        gate.write(v & !0b11);
    });
}
