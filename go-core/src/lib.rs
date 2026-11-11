//! Pure Go (the board game) rules: board state, captures, superko, turns,
//! undo and area scoring. No hardware access and no allocation, so it runs
//! both in the kernel and in host unit tests.
#![cfg_attr(not(test), no_std)]

mod board;
mod game;
mod mc;
mod mcts;
mod playout;
mod rng;
mod score;
mod sgf;
mod zobrist;

pub use board::{Board, Cell, Color, IllegalMove, MAX_SIZE, MoveInfo, Point, PointSet};
pub use game::{Game, Move, Phase};
pub use mc::FlatMc;
pub use mcts::{Mcts, Node};
pub use playout::{is_eye, playout, playout_area, playout_limit, playout_winner, random_bot, random_move, should_pass};
pub use rng::Rng;
pub use score::{Score, score};
pub use sgf::{SgfError, read_sgf, write_sgf};
