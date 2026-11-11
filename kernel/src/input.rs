//! Keyboard input: a lock-free queue of raw scancodes filled by the IRQ1
//! handler, decoded into keys in the main loop.

use pc_keyboard::{DecodedKey, HandleControl, KeyCode, PS2Keyboard, ScancodeSet1, layouts::Us104Key};

use crate::queue::ByteQueue;

static SCANCODES: ByteQueue<128> = ByteQueue::new();

/// Called from the interrupt handler. Drops the scancode if the queue is full.
pub fn push_scancode(code: u8) {
    SCANCODES.push(code);
}

/// True if no scancodes are waiting.
pub fn is_empty() -> bool {
    SCANCODES.is_empty()
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
        while let Some(code) = SCANCODES.pop() {
            if let Ok(Some(event)) = self.decoder.add_byte(code)
                && let Some(key) = self.decoder.process_keyevent(event)
            {
                return Some(key);
            }
        }
        None
    }
}

/// What a key press means to the game.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Up,
    Down,
    Left,
    Right,
    /// Space: place a stone, or toggle a dead group while scoring.
    Place,
    /// Enter: place a stone, or confirm the score.
    Confirm,
    Pass,
    Undo,
    NewGame,
    Escape,
}

pub fn action(key: DecodedKey) -> Option<Action> {
    Some(match key {
        DecodedKey::RawKey(KeyCode::ArrowUp) => Action::Up,
        DecodedKey::RawKey(KeyCode::ArrowDown) => Action::Down,
        DecodedKey::RawKey(KeyCode::ArrowLeft) => Action::Left,
        DecodedKey::RawKey(KeyCode::ArrowRight) => Action::Right,
        DecodedKey::RawKey(KeyCode::Escape) => Action::Escape,
        DecodedKey::RawKey(KeyCode::NumpadEnter) => Action::Confirm,
        DecodedKey::Unicode(c) => match c.to_ascii_lowercase() {
            'w' => Action::Up,
            's' => Action::Down,
            'a' => Action::Left,
            'd' => Action::Right,
            ' ' => Action::Place,
            '\n' | '\r' => Action::Confirm,
            'p' => Action::Pass,
            'u' => Action::Undo,
            'n' => Action::NewGame,
            '\u{1b}' => Action::Escape,
            _ => return None,
        },
        DecodedKey::RawKey(_) => return None,
    })
}
