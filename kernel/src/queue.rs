//! A lock-free single-producer, single-consumer byte queue: an interrupt
//! handler pushes, the main loop pops, and neither ever waits on a lock.

use core::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

pub struct ByteQueue<const N: usize> {
    buf: [AtomicU8; N],
    /// Next slot to read (written only by the consumer).
    head: AtomicUsize,
    /// Next slot to write (written only by the producer).
    tail: AtomicUsize,
}

impl<const N: usize> ByteQueue<N> {
    pub const fn new() -> Self {
        ByteQueue {
            buf: [const { AtomicU8::new(0) }; N],
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    /// Adds a byte; drops it (returning false) if the queue is full.
    pub fn push(&self, byte: u8) -> bool {
        let tail = self.tail.load(Ordering::Relaxed);
        let next = (tail + 1) % N;
        if next == self.head.load(Ordering::Acquire) {
            return false;
        }
        self.buf[tail].store(byte, Ordering::Relaxed);
        self.tail.store(next, Ordering::Release);
        true
    }

    pub fn pop(&self) -> Option<u8> {
        let head = self.head.load(Ordering::Relaxed);
        if head == self.tail.load(Ordering::Acquire) {
            return None;
        }
        let byte = self.buf[head].load(Ordering::Relaxed);
        self.head.store((head + 1) % N, Ordering::Release);
        Some(byte)
    }

    pub fn is_full(&self) -> bool {
        (self.tail.load(Ordering::Relaxed) + 1) % N == self.head.load(Ordering::Acquire)
    }

    pub fn is_empty(&self) -> bool {
        self.head.load(Ordering::Relaxed) == self.tail.load(Ordering::Acquire)
    }
}
