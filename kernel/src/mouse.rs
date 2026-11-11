//! PS/2 mouse input: the IRQ12 handler queues raw bytes, and the main loop
//! assembles them into 3-byte movement packets.

use core::sync::atomic::{AtomicU32, Ordering};

use crate::queue::ByteQueue;

static BYTES: ByteQueue<256> = ByteQueue::new();
/// Bytes lost because the queue was full; the decoder starts a fresh
/// packet when this changes, so packets stay aligned.
static DROPPED: AtomicU32 = AtomicU32::new(0);

/// Called from the interrupt handler.
pub fn push_byte(byte: u8) {
    if !BYTES.push(byte) {
        DROPPED.fetch_add(1, Ordering::Relaxed);
    }
}

pub fn is_empty() -> bool {
    BYTES.is_empty()
}

#[derive(Clone, Copy, Debug)]
pub struct MouseEvent {
    /// Movement since the last packet; `dy` is positive upwards.
    pub dx: i16,
    pub dy: i16,
    pub left: bool,
}

/// First packet byte: buttons, sign bits and overflow flags.
const LEFT: u8 = 1 << 0;
/// Always set in the first byte; used to find packet boundaries again
/// if a byte was lost.
const ALWAYS_ONE: u8 = 1 << 3;
const X_SIGN: u8 = 1 << 4;
const Y_SIGN: u8 = 1 << 5;
const OVERFLOW: u8 = 0b1100_0000;

/// Assembles queued bytes into packets. There must be only one, as it is
/// the queue's only consumer.
pub struct Mouse {
    packet: [u8; 3],
    len: usize,
    /// `DROPPED` when the current packet was started.
    dropped: u32,
}

impl Mouse {
    pub const fn new() -> Self {
        Mouse { packet: [0; 3], len: 0, dropped: 0 }
    }

    pub fn next_event(&mut self) -> Option<MouseEvent> {
        while let Some(byte) = BYTES.pop() {
            let dropped = DROPPED.load(Ordering::Relaxed);
            if dropped != self.dropped {
                // Bytes went missing: whatever we had is garbage.
                self.dropped = dropped;
                self.len = 0;
            }
            if self.len == 0 && byte & ALWAYS_ONE == 0 {
                continue; // out of sync: wait for a plausible first byte
            }
            self.packet[self.len] = byte;
            self.len += 1;
            if self.len < 3 {
                continue;
            }
            self.len = 0;
            let [flags, x, y] = self.packet;
            if flags & OVERFLOW != 0 {
                continue;
            }
            // 9-bit two's complement: the sign bit lives in the flags.
            let extend = |v: u8, negative: bool| v as i16 - if negative { 256 } else { 0 };
            return Some(MouseEvent {
                dx: extend(x, flags & X_SIGN != 0),
                dy: extend(y, flags & Y_SIGN != 0),
                left: flags & LEFT != 0,
            });
        }
        None
    }
}
