#![no_std]
#![no_main]

mod framebuffer;
mod serial;

use bootloader_api::{BootInfo, BootloaderConfig, entry_point};
use core::panic::PanicInfo;
use framebuffer::{Color, Screen};

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

    halt_loop()
}

fn halt_loop() -> ! {
    loop {
        x86_64::instructions::hlt();
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
