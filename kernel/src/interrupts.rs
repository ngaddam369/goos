//! IDT, the 8259 PIC pair and the timer/keyboard interrupt handlers.
//!
//! Handlers stay tiny: they bump a counter or queue a scancode and send EOI.
//! All decoding and drawing happens in the main loop.

use core::sync::atomic::{AtomicU64, Ordering};

use pic8259::ChainedPics;
use spin::{LazyLock, Mutex};
use x86_64::instructions::port::Port;
use x86_64::registers::control::Cr2;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};

use crate::{gdt, input, pit, serial_println};

/// Timer interrupts per second.
pub const TICK_HZ: u64 = 1000;

pub const PIC_1_OFFSET: u8 = 32;
pub const PIC_2_OFFSET: u8 = PIC_1_OFFSET + 8;

// SAFETY: vectors 32..48 don't overlap CPU exceptions.
static PICS: Mutex<ChainedPics> = Mutex::new(unsafe { ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET) });

#[derive(Clone, Copy)]
#[repr(u8)]
enum Irq {
    Timer = PIC_1_OFFSET,
    Keyboard,
}

static TICKS: AtomicU64 = AtomicU64::new(0);

/// Timer ticks since boot (`TICK_HZ` per second).
pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

static IDT: LazyLock<InterruptDescriptorTable> = LazyLock::new(|| {
    let mut idt = InterruptDescriptorTable::new();
    idt.breakpoint.set_handler_fn(breakpoint_handler);
    idt.page_fault.set_handler_fn(page_fault_handler);
    idt.general_protection_fault.set_handler_fn(general_protection_handler);
    idt.invalid_opcode.set_handler_fn(invalid_opcode_handler);
    // SAFETY: the IST index refers to a valid stack set up in gdt.rs.
    unsafe {
        idt.double_fault
            .set_handler_fn(double_fault_handler)
            .set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX);
    }
    idt[Irq::Timer as u8].set_handler_fn(timer_handler);
    idt[Irq::Keyboard as u8].set_handler_fn(keyboard_handler);
    idt[PIC_1_OFFSET + 7].set_handler_fn(spurious_master_handler);
    idt[PIC_2_OFFSET + 7].set_handler_fn(spurious_slave_handler);
    idt
});

/// Sets up the GDT, IDT, PIC and PIT, then enables interrupts.
pub fn init() {
    gdt::init();
    IDT.load();
    pit::set_timer_hz(TICK_HZ as u32);
    // SAFETY: the PICs are remapped away from the CPU exception vectors.
    unsafe {
        let mut pics = PICS.lock();
        pics.initialize();
        // Firmware may leave every line masked; enable only timer (IRQ0)
        // and keyboard (IRQ1).
        pics.write_masks(0b1111_1100, 0b1111_1111);
    }
    x86_64::instructions::interrupts::enable();
}

fn end_of_interrupt(irq: Irq) {
    // SAFETY: called at the end of the handler for that same interrupt.
    unsafe { PICS.lock().notify_end_of_interrupt(irq as u8) };
}

extern "x86-interrupt" fn breakpoint_handler(frame: InterruptStackFrame) {
    serial_println!("EXCEPTION: breakpoint\n{frame:#?}");
}

extern "x86-interrupt" fn double_fault_handler(frame: InterruptStackFrame, _code: u64) -> ! {
    panic!("EXCEPTION: double fault\n{frame:#?}");
}

extern "x86-interrupt" fn page_fault_handler(frame: InterruptStackFrame, code: PageFaultErrorCode) {
    panic!(
        "EXCEPTION: page fault at {:#x} ({code:?})\n{frame:#?}",
        Cr2::read_raw()
    );
}

extern "x86-interrupt" fn general_protection_handler(frame: InterruptStackFrame, code: u64) {
    panic!("EXCEPTION: general protection fault (error code {code:#x})\n{frame:#?}");
}

extern "x86-interrupt" fn invalid_opcode_handler(frame: InterruptStackFrame) {
    panic!("EXCEPTION: invalid opcode\n{frame:#?}");
}

// IRQ7 and IRQ15 are masked, so these vectors can only be spurious
// interrupts from the 8259. A spurious IRQ7 must not be acknowledged; a
// spurious IRQ15 still needs an EOI on the master for the cascade line.
extern "x86-interrupt" fn spurious_master_handler(_frame: InterruptStackFrame) {}

extern "x86-interrupt" fn spurious_slave_handler(_frame: InterruptStackFrame) {
    // SAFETY: 0x20 is the master PIC command port; 0x20 is the EOI command.
    unsafe { Port::<u8>::new(0x20).write(0x20) };
}

extern "x86-interrupt" fn timer_handler(_frame: InterruptStackFrame) {
    TICKS.fetch_add(1, Ordering::Relaxed);
    end_of_interrupt(Irq::Timer);
}

extern "x86-interrupt" fn keyboard_handler(_frame: InterruptStackFrame) {
    // SAFETY: port 0x60 is the PS/2 controller data port.
    let scancode: u8 = unsafe { Port::new(0x60).read() };
    input::push_scancode(scancode);
    end_of_interrupt(Irq::Keyboard);
}
