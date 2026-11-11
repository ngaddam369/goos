//! Global Descriptor Table with a Task State Segment, so the double-fault
//! handler can run on its own known-good stack (e.g. after a stack overflow).

use spin::LazyLock;
use x86_64::VirtAddr;
use x86_64::instructions::segmentation::{CS, DS, ES, SS, Segment};
use x86_64::instructions::tables::load_tss;
use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;

pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;

const STACK_SIZE: usize = 4096 * 5;

#[repr(align(16))]
struct Stack(#[expect(dead_code, reason = "only its address is used")] [u8; STACK_SIZE]);

static TSS: LazyLock<TaskStateSegment> = LazyLock::new(|| {
    static mut DOUBLE_FAULT_STACK: Stack = Stack([0; STACK_SIZE]);
    let mut tss = TaskStateSegment::new();
    // Stacks grow down, so the IST entry points at the top.
    tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] =
        VirtAddr::from_ptr(&raw const DOUBLE_FAULT_STACK) + STACK_SIZE as u64;
    tss
});

struct Selectors {
    code: SegmentSelector,
    data: SegmentSelector,
    tss: SegmentSelector,
}

static GDT: LazyLock<(GlobalDescriptorTable, Selectors)> = LazyLock::new(|| {
    let mut gdt = GlobalDescriptorTable::new();
    let code = gdt.append(Descriptor::kernel_code_segment());
    let data = gdt.append(Descriptor::kernel_data_segment());
    let tss = gdt.append(Descriptor::tss_segment(&TSS));
    (gdt, Selectors { code, data, tss })
});

pub fn init() {
    let (gdt, sel) = &*GDT;
    gdt.load();
    // SAFETY: the selectors come from the GDT that was just loaded. SS and the
    // data segments are reloaded too, because the bootloader's selectors index
    // its own (now replaced) GDT and would fault on the next `iretq`.
    unsafe {
        CS::set_reg(sel.code);
        SS::set_reg(sel.data);
        DS::set_reg(sel.data);
        ES::set_reg(sel.data);
        load_tss(sel.tss);
    }
}
