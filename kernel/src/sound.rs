//! Short, non-blocking PC speaker beeps: a tone is started and switched off
//! by the main loop once its time is up.

use crate::interrupts::TICK_HZ;
use crate::pit;

#[derive(Clone, Copy)]
pub enum Effect {
    /// A stone was placed.
    Click,
    /// A stone was placed and captured something.
    Capture,
    /// The move was rejected.
    Illegal,
}

pub struct Sound {
    /// Tick at which the current tone stops.
    off_at: Option<u64>,
}

impl Sound {
    pub const fn new() -> Self {
        Sound { off_at: None }
    }

    pub fn play(&mut self, effect: Effect, now: u64) {
        let (hz, ms) = match effect {
            Effect::Click => (1800, 12),
            Effect::Capture => (900, 45),
            Effect::Illegal => (220, 140),
        };
        pit::speaker_on(hz);
        self.off_at = Some(now + ms * TICK_HZ / 1000);
    }

    /// Call regularly; silences the speaker when the tone is over.
    pub fn update(&mut self, now: u64) {
        if let Some(t) = self.off_at
            && now >= t
        {
            pit::speaker_off();
            self.off_at = None;
        }
    }
}
