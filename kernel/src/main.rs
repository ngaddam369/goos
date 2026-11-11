#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

mod framebuffer;
mod gdt;
mod input;
mod interrupts;
mod pit;
mod serial;

use bootloader_api::{BootInfo, BootloaderConfig, entry_point};
use core::panic::PanicInfo;
use framebuffer::{Color, Screen};
use interrupts::{TICK_HZ, ticks};

pub static BOOTLOADER_CONFIG: BootloaderConfig = {
    let mut config = BootloaderConfig::new_default();
    config.kernel_stack_size = 256 * 1024;
    config
};

entry_point!(kernel_main, config = &BOOTLOADER_CONFIG);

/// Board wood colour (kaya-like).
pub const WOOD: Color = Color::rgb(0xDC, 0xB3, 0x5C);

fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    serial::init();
    serial_println!("GoOS booted");

    let fb = boot_info.framebuffer.as_mut().expect("no framebuffer from bootloader");
    let mut screen = Screen::new(fb);
    serial_println!("framebuffer {}x{} {:?}", screen.width, screen.height, screen.format());
    screen.clear(WOOD);

    interrupts::init();
    serial_println!("interrupts enabled");

    let mut keyboard = input::Keyboard::new();
    let mut next_report = TICK_HZ;
    loop {
        while let Some(key) = keyboard.next_key() {
            serial_println!("key: {key:?}");
        }
        let now = ticks();
        if now >= next_report {
            serial_println!("tick {now} ({} s)", now / TICK_HZ);
            next_report += TICK_HZ;
        }
        idle();
    }
}

/// Sleeps until the next interrupt, unless a key arrived since we last looked.
fn idle() {
    use x86_64::instructions::interrupts;
    interrupts::disable();
    if input::is_empty() {
        // `sti; hlt` back to back: an interrupt can't slip in between.
        interrupts::enable_and_hlt();
    } else {
        interrupts::enable();
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    serial::print_panic(format_args!("KERNEL PANIC: {info}\n"));
    loop {
        x86_64::instructions::interrupts::disable();
        x86_64::instructions::hlt();
    }
}
