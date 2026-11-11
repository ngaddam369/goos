#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

mod app;
mod bot;
mod font;
mod framebuffer;
mod gdt;
mod input;
mod interrupts;
mod mouse;
mod pit;
mod pointer;
mod ps2;
mod queue;
mod serial;
mod sound;
mod ui;

use bootloader_api::{BootInfo, BootloaderConfig, entry_point};
use core::panic::PanicInfo;
use framebuffer::{Canvas, Screen};
use interrupts::ticks;

pub static BOOTLOADER_CONFIG: BootloaderConfig = {
    let mut config = BootloaderConfig::new_default();
    config.kernel_stack_size = 256 * 1024;
    config
};

entry_point!(kernel_main, config = &BOOTLOADER_CONFIG);

fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    serial::init();
    serial_println!("GoOS booted");

    let fb = boot_info.framebuffer.as_mut().expect("no framebuffer from bootloader");
    let screen = Screen::new(fb);
    serial_println!("framebuffer {}x{} {:?}", screen.width(), screen.height(), screen.format());

    let has_mouse = interrupts::init();
    serial_println!("mouse: {}", if has_mouse { "found" } else { "none" });
    let mut keyboard = input::Keyboard::new();
    let mut mouse = mouse::Mouse::new();
    let mut app = app::App::new(screen);
    // The runner waits for this before sending a game record to load.
    serial_println!("GoOS ready");
    loop {
        let now = ticks();
        while let Some(key) = keyboard.next_key() {
            if let Some(action) = input::action(key) {
                app.handle(action, now);
            }
        }
        while let Some(event) = mouse.next_event() {
            app.handle_mouse(event, now);
        }
        serial::poll();
        while let Some(byte) = serial::receive() {
            app.handle_serial(byte, now);
        }
        app.update(now);
        if !app.busy() {
            idle();
        }
    }
}

/// Sleeps until the next interrupt, unless a key arrived since we last looked.
fn idle() {
    use x86_64::instructions::interrupts;
    interrupts::disable();
    if input::is_empty() && mouse::is_empty() && !serial::has_input() {
        // `sti; hlt` back to back: an interrupt can't slip in between.
        interrupts::enable_and_hlt();
    } else {
        interrupts::enable();
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    // Don't leave a beep sounding forever.
    pit::speaker_off();
    serial::print_panic(format_args!("KERNEL PANIC: {info}\n"));
    loop {
        x86_64::instructions::interrupts::disable();
        x86_64::instructions::hlt();
    }
}
