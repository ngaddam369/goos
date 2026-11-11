//! COM1 serial output for debugging (`serial_println!`).

use core::fmt::{self, Write};

use spin::Mutex;
use uart_16550::{Config, Uart16550Tty, backend::PioBackend};
use x86_64::instructions::interrupts;

const COM1: u16 = 0x3F8;

static SERIAL: Mutex<Option<Uart16550Tty<PioBackend>>> = Mutex::new(None);

pub fn init() {
    // SAFETY: COM1 is the standard serial port on PC hardware and QEMU, and
    // nothing else in the kernel touches these I/O ports.
    let port = unsafe { Uart16550Tty::new_port(COM1, Config::default()) };
    // If there's no working UART, printing is silently skipped.
    *SERIAL.lock() = port.ok();
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    // Interrupt handlers may print too, so never hold the lock with
    // interrupts enabled (that could deadlock).
    interrupts::without_interrupts(|| {
        if let Some(port) = SERIAL.lock().as_mut() {
            let _ = port.write_fmt(args);
        }
    });
}

/// Used by the panic handler: the panic may have happened while the lock was
/// held, so take it by force rather than spinning forever.
pub fn print_panic(args: fmt::Arguments) {
    interrupts::disable();
    // SAFETY: interrupts are off and we never return to the code that held
    // the lock, so nobody else can be using the port.
    unsafe { SERIAL.force_unlock() };
    if let Some(port) = SERIAL.lock().as_mut() {
        let _ = port.write_fmt(args);
    }
}

#[macro_export]
macro_rules! serial_print {
    ($($arg:tt)*) => ($crate::serial::_print(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! serial_println {
    () => ($crate::serial_print!("\n"));
    ($($arg:tt)*) => ($crate::serial_print!("{}\n", format_args!($($arg)*)));
}
