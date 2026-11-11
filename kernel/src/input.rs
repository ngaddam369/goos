//! Keyboard input: a lock-free queue of raw scancodes filled by the IRQ1
//! handler, decoded into keys in the main loop.

use core::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

use pc_keyboard::{DecodedKey, HandleControl, PS2Keyboard, ScancodeSet1, layouts::Us104Key};

const QUEUE_LEN: usize = 128;

// Single producer (the keyboard interrupt) and single consumer (the main
// loop), so plain atomics suffice and the handler never waits on a lock.
static QUEUE: [AtomicU8; QUEUE_LEN] = [const { AtomicU8::new(0) }; QUEUE_LEN];
static HEAD: AtomicUsize = AtomicUsize::new(0); // next slot to read
static TAIL: AtomicUsize = AtomicUsize::new(0); // next slot to write

/// Called from the interrupt handler. Drops the scancode if the queue is full.
pub fn push_scancode(code: u8) {
    let tail = TAIL.load(Ordering::Relaxed);
    let next = (tail + 1) % QUEUE_LEN;
    if next == HEAD.load(Ordering::Acquire) {
        return;
    }
    QUEUE[tail].store(code, Ordering::Relaxed);
    TAIL.store(next, Ordering::Release);
}

/// True if no scancodes are waiting.
pub fn is_empty() -> bool {
    HEAD.load(Ordering::Relaxed) == TAIL.load(Ordering::Acquire)
}

fn pop_scancode() -> Option<u8> {
    let head = HEAD.load(Ordering::Relaxed);
    if head == TAIL.load(Ordering::Acquire) {
        return None;
    }
    let code = QUEUE[head].load(Ordering::Relaxed);
    HEAD.store((head + 1) % QUEUE_LEN, Ordering::Release);
    Some(code)
}

/// Turns queued scancodes into key presses (handles multi-byte arrow keys).
/// There must be only one of these, as it is the queue's only consumer.
pub struct Keyboard {
    decoder: PS2Keyboard<Us104Key, ScancodeSet1>,
}

impl Keyboard {
    pub const fn new() -> Self {
        Keyboard {
            decoder: PS2Keyboard::new(ScancodeSet1::new(), Us104Key, HandleControl::Ignore),
        }
    }

    /// The next decoded key press, if any is waiting.
    pub fn next_key(&mut self) -> Option<DecodedKey> {
        while let Some(code) = pop_scancode() {
            if let Ok(Some(event)) = self.decoder.add_byte(code)
                && let Some(key) = self.decoder.process_keyevent(event)
            {
                return Some(key);
            }
        }
        None
    }
}
