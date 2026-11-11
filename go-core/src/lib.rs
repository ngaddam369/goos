//! Pure Go (the board game) rules: board state, captures, superko, turns,
//! undo and area scoring. No hardware access and no allocation, so it runs
//! both in the kernel and in host unit tests.
#![cfg_attr(not(test), no_std)]

mod board;
mod game;
mod score;
mod zobrist;

pub use board::{Board, Cell, Color, IllegalMove, MAX_SIZE, MoveInfo, Point, PointSet};
pub use game::{Game, Move, Phase};
pub use score::{Score, score};
