//! Turns the kernel ELF (built by cargo as an artifact dependency) into
//! bootable BIOS and UEFI disk images.

use std::path::PathBuf;

use bootloader::{BiosBoot, BootConfig, UefiBoot};

fn main() {
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let kernel = PathBuf::from(std::env::var_os("CARGO_BIN_FILE_KERNEL_kernel").unwrap());

    let mut config = BootConfig::default();
    // The kernel paints the whole screen itself; skip the bootloader's
    // on-screen log so boot is faster and there's no text flash.
    config.frame_buffer_logging = false;
    // Only UEFI honours this; BIOS picks the best VESA mode up to 1280x720.
    config.frame_buffer.minimum_framebuffer_width = Some(1280);
    config.frame_buffer.minimum_framebuffer_height = Some(720);

    let bios_path = out_dir.join("bios.img");
    BiosBoot::new(&kernel)
        .set_boot_config(&config)
        .create_disk_image(&bios_path)
        .expect("failed to create BIOS disk image");

    let uefi_path = out_dir.join("uefi.img");
    UefiBoot::new(&kernel)
        .set_boot_config(&config)
        .create_disk_image(&uefi_path)
        .expect("failed to create UEFI disk image");

    println!("cargo:rustc-env=BIOS_PATH={}", bios_path.display());
    println!("cargo:rustc-env=UEFI_PATH={}", uefi_path.display());
}
