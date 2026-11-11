//! COM1 serial port: output for debugging (`serial_println!`) and game
//! records, and input (received bytes are queued by the IRQ4 handler).

use core::fmt::{self, Write};
use core::sync::atomic::{AtomicBool, Ordering};

use spin::Mutex;
use uart_16550::{BaudRate, Config, Uart16550Tty, backend::PioBackend};
use x86_64::instructions::interrupts;
use x86_64::instructions::port::Port;

use crate::queue::ByteQueue;

const COM1: u16 = 0x3F8;
/// UART registers (offsets from the base port).
const RECEIVE: u16 = COM1;
const INTERRUPT_ENABLE: u16 = COM1 + 1;
const MODEM_CONTROL: u16 = COM1 + 4;
const LINE_STATUS: u16 = COM1 + 5;
/// Interrupt-enable bit: data received.
const DATA_AVAILABLE_IRQ: u8 = 1 << 0;
/// Modem-control bit that connects the UART's interrupt line on PCs.
const OUT2: u8 = 1 << 3;
/// Line-status bit: a received byte is waiting.
const DATA_READY: u8 = 1 << 0;

static RECEIVED: ByteQueue<4096> = ByteQueue::new();
/// Reception is paused because the queue is full (flow control: bytes
/// wait in the UART, and QEMU holds back the rest).
static PAUSED: AtomicBool = AtomicBool::new(false);
/// Receive interrupts are wanted (there is a working UART).
static RECEIVING: AtomicBool = AtomicBool::new(false);

static SERIAL: Mutex<Option<Uart16550Tty<PioBackend>>> = Mutex::new(None);

pub fn init() {
    // SAFETY: COM1 is the standard serial port on PC hardware and QEMU, and
    // nothing else in the kernel touches these I/O ports.
    // Printing runs with interrupts off, so use the fastest common baud rate.
    let config = Config { baud_rate: BaudRate::Baud115200, ..Config::DEFAULT };
    let port = unsafe { Uart16550Tty::new_port(COM1, config) };
    // If there's no working UART, printing is silently skipped.
    *SERIAL.lock() = port.ok();
}

/// Turns on the "byte received" interrupt (IRQ4). Returns false if there
/// is no working UART. Call with interrupts disabled.
pub fn enable_receive_interrupt() -> bool {
    if SERIAL.lock().is_none() {
        return false;
    }
    // SAFETY: COM1 registers; the UART driver doesn't use the interrupt
    // enable or modem control registers after init.
    unsafe {
        Port::<u8>::new(INTERRUPT_ENABLE).write(DATA_AVAILABLE_IRQ);
        let mut mcr = Port::<u8>::new(MODEM_CONTROL);
        let v = mcr.read();
        mcr.write(v | OUT2);
    }
    RECEIVING.store(true, Ordering::Relaxed);
    true
}

fn set_receive_interrupt(on: bool) {
    // SAFETY: COM1's interrupt enable register, only touched here.
    unsafe { Port::<u8>::new(INTERRUPT_ENABLE).write(if on { DATA_AVAILABLE_IRQ } else { 0 }) };
}

/// Moves the bytes waiting in the UART into the receive queue. Called by
/// the IRQ4 handler, and with interrupts off from `poll`. If the queue
/// fills up, reception pauses: the bytes stay in the UART and its
/// interrupt is switched off until `poll` finds room again.
pub fn take_received() {
    if !RECEIVING.load(Ordering::Relaxed) {
        return;
    }
    // Bounded, in case the line status reads garbage.
    for _ in 0..4096 {
        if RECEIVED.is_full() {
            set_receive_interrupt(false);
            PAUSED.store(true, Ordering::Relaxed);
            return;
        }
        // SAFETY: reading the line status and receive registers of COM1.
        unsafe {
            if Port::<u8>::new(LINE_STATUS).read() & DATA_READY == 0 {
                return;
            }
            RECEIVED.push(Port::<u8>::new(RECEIVE).read());
        }
    }
}

/// Called on every pass of the main loop: resumes paused reception and
/// collects bytes whose interrupt was missed (IRQ4 is edge-triggered, so a
/// byte left behind by the handler would otherwise never be read).
pub fn poll() {
    interrupts::without_interrupts(|| {
        if PAUSED.load(Ordering::Relaxed) && !RECEIVED.is_full() {
            PAUSED.store(false, Ordering::Relaxed);
            set_receive_interrupt(true);
        }
        if !PAUSED.load(Ordering::Relaxed) {
            take_received();
        }
    });
}

/// The next received byte, if any.
pub fn receive() -> Option<u8> {
    RECEIVED.pop()
}

pub fn has_input() -> bool {
    !RECEIVED.is_empty()
}

/// `core::fmt::Write` to the serial port, for longer outputs.
pub struct SerialOut;

impl Write for SerialOut {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        _print(format_args!("{s}"));
        Ok(())
    }
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
