//! Programmable Interval Timer (8253/8254).

use x86_64::instructions::port::Port;

/// Input clock of the PIT in Hz.
const PIT_HZ: u32 = 1_193_182;

const CHANNEL0: u16 = 0x40;
const COMMAND: u16 = 0x43;

/// Makes channel 0 (wired to IRQ0) fire `hz` times per second.
pub fn set_timer_hz(hz: u32) {
    let divisor = (PIT_HZ / hz) as u16;
    // SAFETY: standard PIT ports; called once during init with interrupts off.
    unsafe {
        // Channel 0, low byte then high byte, mode 3 (square wave), binary.
        Port::<u8>::new(COMMAND).write(0x36);
        let mut data = Port::<u8>::new(CHANNEL0);
        data.write(divisor as u8);
        data.write((divisor >> 8) as u8);
    }
}
