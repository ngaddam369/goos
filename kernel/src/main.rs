#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

mod font;
mod framebuffer;
mod gdt;
mod input;
mod interrupts;
mod pit;
mod serial;
mod ui;

use bootloader_api::{BootInfo, BootloaderConfig, entry_point};
use core::panic::PanicInfo;
use framebuffer::{Canvas, Screen};
use interrupts::{TICK_HZ, ticks};

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
    let mut screen = Screen::new(fb);
    serial_println!("framebuffer {}x{} {:?}", screen.width(), screen.height(), screen.format());
    draw_demo(&mut screen);

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

/// M3 demo: a 19x19 board with a few hard-coded stones.
fn draw_demo(screen: &mut Screen) {
    use ui::{PointView, Stone};
    const STONES: &[(usize, usize, Stone)] = &[
        (3, 3, Stone::Black),
        (15, 15, Stone::White),
        (15, 3, Stone::Black),
        (3, 15, Stone::White),
        (2, 5, Stone::Black),
        (16, 13, Stone::White),
        (9, 9, Stone::Black),
        (0, 0, Stone::White),
        (18, 18, Stone::Black),
        (10, 9, Stone::White),
    ];
    let layout = ui::Layout::new(screen.width(), screen.height(), 19);
    ui::draw_board(screen, &layout, |col, row| PointView {
        stone: STONES.iter().find(|s| s.0 == col && s.1 == row).map(|s| s.2),
    });
    ui::panel_line(screen, &layout, 0, "GoOS", layout.title_font, ui::TEXT);
    ui::panel_line(screen, &layout, 2, "Black to play", layout.text_font, ui::TEXT);
    ui::panel_line(screen, &layout, 3, "Captures  B 0  W 0", layout.text_font, ui::DIM_TEXT);
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
