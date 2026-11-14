//! The game controller: turns key actions into moves on the `go-core` game
//! and keeps the screen in sync with it.
//!
//! Rendering is driven by state, not by events: after anything changes, the
//! view of every intersection and every status line is recomputed and only
//! those that differ from what is on screen get redrawn.

use core::fmt::Write;

use go_core::{
    Cell, Color as Player, Game, MAX_SIZE, Move, Phase, Point, Score, SgfError, read_sgf, write_sgf,
};
use spin::{Mutex, MutexGuard};

use crate::bot::{self, Bot};
use crate::font::{Font, TextBuf};
use crate::framebuffer::{Canvas, Color, Screen};
use crate::input::Action;
use crate::interrupts::TICK_HZ;
use crate::mouse::MouseEvent;
use crate::pointer::Pointer;
use crate::serial::SerialOut;
use crate::serial_println;
use crate::sound::{Effect, Sound};
use crate::ui::{self, Layout, PointView};

/// The game lives in a static, built at compile time: with its undo
/// history it's bigger than the kernel stack.
static GAME: Mutex<Game> = Mutex::new(Game::new(19));

const SIZES: [usize; 3] = [9, 13, 19];

/// Who plays the other side.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Opponent {
    Human,
    /// The computer plays this colour.
    Computer(Player),
}

const OPPONENTS: [Opponent; 3] = [
    Opponent::Human,
    Opponent::Computer(Player::White),
    Opponent::Computer(Player::Black),
];
/// How long an illegal-move message stays up.
const NOTICE_TICKS: u64 = TICK_HZ * 3 / 2;
const MAX_PANEL_LINES: usize = 24;
const COLUMNS: &[u8; 19] = b"ABCDEFGHJKLMNOPQRST";

type Line = TextBuf<40>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Choosing the board size.
    Start,
    Game,
}

/// A short-lived message in the status panel (illegal move etc.).
struct Notice {
    text: Line,
    color: Color,
    until: u64,
}

/// Marker lines around a game record sent over serial; the runner looks
/// for them to save the record to a file.
const SGF_BEGIN: &str = "-----BEGIN SGF-----";
const SGF_END: &str = "-----END SGF-----";

/// Longest game record accepted over serial.
const SGF_MAX: usize = 32 * 1024;

/// Collects bytes arriving on serial until they form one complete SGF game
/// tree: from a `(` to its matching `)`, ignoring brackets inside property
/// values (which may contain escaped `]`).
struct SgfReceiver {
    buf: [u8; SGF_MAX],
    len: usize,
    /// Inside a record (`len` counts its bytes unless `discarding`).
    active: bool,
    /// The record was too long: skip the rest of it rather than storing it.
    discarding: bool,
    depth: u32,
    in_value: bool,
    escaped: bool,
}

enum Received {
    Nothing,
    /// A complete tree of this many bytes is in `buf`.
    Complete(usize),
    TooLong,
}

impl SgfReceiver {
    const fn new() -> Self {
        SgfReceiver {
            buf: [0; SGF_MAX],
            len: 0,
            active: false,
            discarding: false,
            depth: 0,
            in_value: false,
            escaped: false,
        }
    }

    fn reset(&mut self) {
        self.len = 0;
        self.active = false;
        self.discarding = false;
        self.depth = 0;
        self.in_value = false;
        self.escaped = false;
    }

    fn feed(&mut self, byte: u8) -> Received {
        if !self.active {
            if byte != b'(' {
                return Received::Nothing; // noise between records
            }
            self.active = true;
        }
        let mut result = Received::Nothing;
        if !self.discarding {
            if self.len == SGF_MAX {
                // Keep following the structure to find the record's end, so
                // nothing inside it is mistaken for the start of another.
                self.discarding = true;
                result = Received::TooLong;
            } else {
                self.buf[self.len] = byte;
                self.len += 1;
            }
        }
        if self.in_value {
            match byte {
                _ if self.escaped => self.escaped = false,
                b'\\' => self.escaped = true,
                b']' => self.in_value = false,
                _ => {}
            }
        } else {
            match byte {
                b'[' => self.in_value = true,
                b'(' => self.depth += 1,
                b')' => {
                    self.depth -= 1;
                    if self.depth == 0 {
                        let (len, complete) = (self.len, !self.discarding);
                        self.reset();
                        if complete {
                            return Received::Complete(len);
                        }
                    }
                }
                _ => {}
            }
        }
        result
    }
}

pub struct App {
    screen: Screen,
    game: MutexGuard<'static, Game>,
    mode: Mode,
    /// Index into `SIZES` of the size picked on the start screen.
    size_choice: usize,
    /// Index into `OPPONENTS` of the opponent picked on the start screen.
    opponent_choice: usize,
    /// Which start screen row (0: size, 1: opponent) the arrows change.
    start_row: usize,
    /// The opponent in the current game.
    opponent: Opponent,
    bot: Bot,
    /// A game has been started (so Esc on the start screen returns to it).
    has_game: bool,
    layout: Layout,
    cursor: Point,
    notice: Option<Notice>,
    /// What each intersection currently shows on screen (`None`: unknown,
    /// must be drawn).
    drawn: [Option<PointView>; MAX_SIZE * MAX_SIZE],
    /// What each status panel line currently shows.
    panel: [Option<(Line, Color)>; MAX_PANEL_LINES],
    /// Something changed since the last render.
    dirty: bool,
    /// The whole screen must be repainted (new game or mode switch).
    repaint: bool,
    sound: Sound,
    /// Thinking time used by black and white, in ticks.
    clocks: [u64; 2],
    /// Tick up to which the clocks have been charged.
    clock_mark: u64,
    /// Seconds shown on the running clock at the last render.
    shown_secs: u64,
    /// When to next refresh the "thinking" progress line.
    next_progress: u64,
    /// Mouse pointer position; `None` until the mouse is first used.
    mouse_at: Option<(usize, usize)>,
    /// The left button was down in the last mouse packet.
    left_down: bool,
    /// The intersection under the pointer, last time it moved.
    hovered: Option<Point>,
    pointer: Pointer,
    /// The pointer moved since it was last drawn.
    pointer_moved: bool,
    sgf_in: SgfReceiver,
}

impl App {
    pub fn new(screen: Screen) -> Self {
        let layout = Layout::new(screen.width(), screen.height(), 19);
        App {
            screen,
            game: GAME.lock(),
            mode: Mode::Start,
            size_choice: 2,
            opponent_choice: 0,
            start_row: 0,
            opponent: Opponent::Human,
            bot: Bot::new(bot::seed()),
            has_game: false,
            layout,
            cursor: Point::new(9, 9),
            notice: None,
            drawn: [None; MAX_SIZE * MAX_SIZE],
            panel: [const { None }; MAX_PANEL_LINES],
            dirty: true,
            repaint: true,
            sound: Sound::new(),
            clocks: [0; 2],
            clock_mark: 0,
            shown_secs: 0,
            next_progress: 0,
            mouse_at: None,
            left_down: false,
            hovered: None,
            pointer: Pointer::new(),
            pointer_moved: false,
            sgf_in: SgfReceiver::new(),
        }
    }

    pub fn handle(&mut self, action: Action, now: u64) {
        self.dirty = true;
        match self.mode {
            Mode::Start => self.handle_start(action),
            Mode::Game => self.handle_game(action, now),
        }
    }

    /// Moves the pointer; hovering over the board moves the cursor there and
    /// a left click acts like Space on that point.
    pub fn handle_mouse(&mut self, event: MouseEvent, now: u64) {
        let (w, h) = (self.screen.width() as isize, self.screen.height() as isize);
        let (x, y) = self
            .mouse_at
            .map_or((w / 2, h / 2), |(x, y)| (x as isize, y as isize));
        let x = (x + event.dx as isize).clamp(0, w - 1) as usize;
        let y = (y - event.dy as isize).clamp(0, h - 1) as usize;
        self.mouse_at = Some((x, y));
        self.pointer_moved = true;

        let clicked = event.left && !self.left_down;
        self.left_down = event.left;
        if self.mode != Mode::Game {
            return;
        }
        let hovered = self
            .layout
            .point_at(x, y)
            .map(|(col, row)| Point::new(col, row));
        // Follow the pointer only when it reaches a new intersection, so a
        // nudge doesn't undo cursor moves made with the keyboard.
        if let Some(p) = hovered
            && (hovered != self.hovered || clicked)
            && p != self.cursor
        {
            self.cursor = p;
            self.dirty = true;
        }
        self.hovered = hovered;
        if clicked && hovered.is_some() {
            self.handle(Action::Place, now);
        }
    }

    fn handle_start(&mut self, action: Action) {
        let cycle = |v: &mut usize, len: usize, step: usize| *v = (*v + step) % len;
        match action {
            Action::Up | Action::Down => self.start_row = 1 - self.start_row,
            Action::Left | Action::Right => {
                let step = if action == Action::Right { 1 } else { 2 };
                match self.start_row {
                    0 => cycle(&mut self.size_choice, SIZES.len(), step),
                    _ => cycle(&mut self.opponent_choice, OPPONENTS.len(), step),
                }
            }
            Action::NewGame => cycle(&mut self.size_choice, SIZES.len(), 1),
            Action::Place | Action::Confirm => self.start_game(),
            Action::Escape if self.has_game => {
                self.mode = Mode::Game;
                self.repaint = true;
            }
            _ => {}
        }
    }

    fn start_game(&mut self) {
        let size = SIZES[self.size_choice];
        self.game.reset(size);
        self.layout = Layout::new(self.screen.width(), self.screen.height(), size);
        self.hovered = None;
        self.cursor = Point::new(size / 2, size / 2);
        self.notice = None;
        self.clocks = [0; 2];
        self.opponent = OPPONENTS[self.opponent_choice];
        self.bot.cancel();
        self.mode = Mode::Game;
        self.has_game = true;
        self.repaint = true;
        serial_println!("new game {size}x{size}");
    }

    fn handle_game(&mut self, action: Action, now: u64) {
        let phase = self.game.phase();
        match action {
            Action::Up => self.move_cursor(0, -1),
            Action::Down => self.move_cursor(0, 1),
            Action::Left => self.move_cursor(-1, 0),
            Action::Right => self.move_cursor(1, 0),
            Action::Place | Action::Confirm | Action::Pass
                if phase == Phase::Playing && self.computer_to_move() =>
            {
                self.set_timed_notice("The computer is thinking", now);
            }
            Action::Place | Action::Confirm if phase == Phase::Playing => {
                self.play(self.cursor, now)
            }
            Action::Place if phase == Phase::Scoring => {
                self.game.toggle_dead(self.cursor);
            }
            Action::Confirm if phase == Phase::Scoring => {
                self.game.finish();
                let score = self.game.score();
                let mut result = Line::new();
                write_result(&mut result, &score, true);
                serial_println!("game over: {}", result.as_str());
                self.save_game();
            }
            Action::SaveGame => {
                self.save_game();
                self.set_notice("Game record sent to serial", ui::ACCENT, now);
            }
            Action::Pass if phase == Phase::Playing => self.pass(),
            Action::Undo => {
                if self.game.undo() {
                    self.bot.cancel();
                    // Against the computer, go back to the human's turn.
                    while self.game.phase() == Phase::Playing
                        && self.computer_to_move()
                        && self.game.undo()
                    {}
                    serial_println!("undo");
                    self.notice = None;
                } else {
                    self.set_timed_notice("Nothing to undo", now);
                }
            }
            Action::Escape if phase == Phase::Scoring => {
                self.game.resume();
                self.notice = None;
            }
            Action::NewGame => {
                self.bot.cancel();
                self.mode = Mode::Start;
                self.repaint = true;
            }
            _ => {}
        }
    }

    /// It's the computer's turn.
    fn computer_turn(&self) -> bool {
        self.mode == Mode::Game && self.game.phase() == Phase::Playing && self.computer_to_move()
    }

    /// The computer is thinking: the main loop should keep calling
    /// `update` instead of sleeping.
    pub fn busy(&self) -> bool {
        self.computer_turn() && self.bot.computing()
    }

    fn computer_to_move(&self) -> bool {
        self.opponent == Opponent::Computer(self.game.to_move())
    }

    fn pass(&mut self) {
        serial_println!("{} passes", name(self.game.to_move()));
        let _ = self.game.pass();
        self.notice = None;
    }

    fn play(&mut self, p: Point, now: u64) {
        let player = self.game.to_move();
        match self.game.play(p) {
            Ok(info) => {
                let coord = coordinate(p, self.game.size());
                serial_println!(
                    "{} {} (captured {})",
                    name(player),
                    coord.as_str(),
                    info.captured
                );
                self.notice = None;
                let effect = if info.captured > 0 {
                    Effect::Capture
                } else {
                    Effect::Click
                };
                self.sound.play(effect, now);
            }
            Err(e) => {
                serial_println!("illegal: {}", e.message());
                self.set_timed_notice(e.message(), now);
                self.sound.play(Effect::Illegal, now);
            }
        }
    }

    fn set_timed_notice(&mut self, message: &str, now: u64) {
        self.set_notice(message, ui::WARNING, now);
    }

    fn set_notice(&mut self, message: &str, color: Color, now: u64) {
        let mut text = Line::new();
        let _ = text.write_str(message);
        // Warnings are brief; other news stays up a little longer.
        let duration = if color == ui::WARNING {
            NOTICE_TICKS
        } else {
            2 * NOTICE_TICKS
        };
        self.notice = Some(Notice {
            text,
            color,
            until: now + duration,
        });
    }

    /// Sends the game record (SGF) over serial between marker lines.
    fn save_game(&mut self) {
        serial_println!("{SGF_BEGIN}");
        let _ = write_sgf(&self.game, &mut SerialOut);
        serial_println!("{SGF_END}");
    }

    /// Feeds a byte received on serial to the SGF receiver, and loads the
    /// game once a complete record has arrived.
    pub fn handle_serial(&mut self, byte: u8, now: u64) {
        match self.sgf_in.feed(byte) {
            Received::Nothing => {}
            Received::TooLong => {
                serial_println!("SGF: record too long (over {SGF_MAX} bytes)");
                self.set_timed_notice("Game record too long", now);
                self.dirty = true;
            }
            Received::Complete(len) => self.load_game(len, now),
        }
    }

    fn load_game(&mut self, len: usize, now: u64) {
        let result = read_sgf(&self.sgf_in.buf[..len], &mut self.game);
        let loaded = match result {
            Ok(()) => true,
            // Loaded up to the bad move: show what we have.
            Err(SgfError::Illegal(..) | SgfError::OutOfTurn(_)) => true,
            Err(_) => false,
        };
        match result {
            Ok(()) => serial_println!(
                "SGF: loaded {}x{} game, {} moves",
                self.game.size(),
                self.game.size(),
                self.game.move_number()
            ),
            Err(SgfError::Illegal(n, e)) => serial_println!(
                "SGF: move {n} is illegal ({}); loaded the moves before it",
                e.message()
            ),
            Err(SgfError::OutOfTurn(n)) => {
                serial_println!("SGF: move {n} is out of turn; loaded the moves before it")
            }
            Err(e) => serial_println!("SGF: not loaded: {}", e.message()),
        }
        if loaded {
            // Loaded games are played between two humans.
            let size = self.game.size();
            self.layout = Layout::new(self.screen.width(), self.screen.height(), size);
            self.hovered = None;
            self.cursor = Point::new(size / 2, size / 2);
            self.clocks = [0; 2];
            self.opponent = Opponent::Human;
            self.bot.cancel();
            self.mode = Mode::Game;
            self.has_game = true;
            self.repaint = true;
        }
        let mut text = Line::new();
        match result {
            Ok(()) => {
                let _ = write!(text, "Game loaded: {} moves", self.game.move_number());
                self.set_notice(text.as_str(), ui::ACCENT, now);
            }
            Err(e) => {
                let _ = write!(text, "Game record: {}", e.message());
                self.set_timed_notice(text.as_str(), now);
            }
        }
        self.dirty = true;
    }

    fn move_cursor(&mut self, dx: isize, dy: isize) {
        let max = self.game.size() as isize - 1;
        let col = (self.cursor.col as isize + dx).clamp(0, max);
        let row = (self.cursor.row as isize + dy).clamp(0, max);
        self.cursor = Point::new(col as usize, row as usize);
    }

    /// Runs the clocks, ends finished beeps, expires timed messages and
    /// redraws whatever changed.
    pub fn update(&mut self, now: u64) {
        self.sound.update(now);

        if self.computer_turn() {
            match self.bot.think(&self.game, now) {
                Some(Move::Play(p)) => self.play(p, now),
                Some(Move::Pass) => self.pass(),
                None => {}
            }
            // Refresh the playout counter a few times a second.
            if now >= self.next_progress {
                self.next_progress = now + TICK_HZ / 4;
                self.dirty = true;
            }
        }

        // Only the player to move uses time, and only during play.
        let elapsed = now.saturating_sub(self.clock_mark);
        self.clock_mark = now;
        if self.mode == Mode::Game && self.game.phase() == Phase::Playing {
            let clock = &mut self.clocks[self.game.to_move() as usize];
            *clock += elapsed;
            let secs = *clock / TICK_HZ;
            if secs != self.shown_secs {
                self.shown_secs = secs;
                self.dirty = true;
            }
        }

        if let Some(Notice { until, .. }) = self.notice
            && now >= until
        {
            self.notice = None;
            self.dirty = true;
        }
        if self.dirty || self.pointer_moved {
            // Take the pointer off the screen while drawing under it.
            self.pointer.hide(&mut self.screen);
            if self.dirty {
                self.dirty = false;
                match self.mode {
                    Mode::Start => self.render_start(),
                    Mode::Game => self.render_game(),
                }
            }
            if let Some((x, y)) = self.mouse_at {
                self.pointer.show(&mut self.screen, x, y);
            }
            self.pointer_moved = false;
        }
    }

    fn render_start(&mut self) {
        let screen = &mut self.screen;
        let title = self.layout.title_font;
        let text = self.layout.text_font;
        let line_h = text.line_height() * 2;
        let scale = if screen.height() >= 1000 { 4 } else { 3 };
        let title_h = title.line_height() * scale + 16;
        let title_y = screen.height() / 4;
        let top = title_y + title_h;

        // The static parts are only drawn once, so key presses don't make
        // them flash.
        if self.repaint {
            self.repaint = false;
            screen.clear(ui::BACKGROUND);
            // The board will need a full repaint after this screen.
            self.drawn = [None; MAX_SIZE * MAX_SIZE];
            self.panel = [const { None }; MAX_PANEL_LINES];

            // Big title flanked by a black and a white stone.
            ui::centered_line_scaled(screen, title_y, title_h, "GoOS", title, ui::TEXT, scale);
            let r = (title_h * 3 / 10) as isize;
            let reach = (title.text_width("GoOS") * scale / 2) as isize + 2 * r;
            let (cx, cy) = (
                (screen.width() / 2) as isize,
                (title_y + title_h / 2) as isize,
            );
            ui::draw_stone(screen, cx - reach, cy, r, Player::Black, 255);
            ui::draw_stone(screen, cx + reach, cy, r, Player::White, 255);
            ui::centered_line(screen, top, line_h, "Go on bare metal", text, ui::DIM_TEXT);
            ui::centered_line(
                screen,
                top + 2 * line_h,
                line_h,
                "Board size",
                text,
                ui::TEXT,
            );
            ui::centered_line(screen, top + 4 * line_h, line_h, "Opponent", text, ui::TEXT);
        }

        // The selected row is highlighted and shows arrows.
        let mut value = |row: usize, args: core::fmt::Arguments| {
            let mut line = Line::new();
            let selected = row == self.start_row;
            let _ = if selected {
                write!(line, "<  {args}  >")
            } else {
                write!(line, "{args}")
            };
            let color = if selected { ui::ACCENT } else { ui::TEXT };
            ui::centered_line(
                screen,
                top + (3 + 2 * row) * line_h,
                line_h,
                line.as_str(),
                title,
                color,
            );
        };
        let size = SIZES[self.size_choice];
        value(0, format_args!("{size} x {size}"));
        match OPPONENTS[self.opponent_choice] {
            Opponent::Human => value(1, format_args!("Human")),
            Opponent::Computer(c) => value(1, format_args!("Computer plays {}", name(c))),
        }
        let help = if self.has_game {
            "Up/Down: choose    Left/Right: change    Enter: start    Esc: back"
        } else {
            "Up/Down: choose    Left/Right: change    Enter: start"
        };
        ui::centered_line(screen, top + 7 * line_h, line_h, help, text, ui::DIM_TEXT);
    }

    fn render_game(&mut self) {
        let size = self.game.size();
        let score = match self.game.phase() {
            Phase::Playing => None,
            Phase::Scoring | Phase::Finished => Some(self.game.score()),
        };

        if self.repaint {
            self.repaint = false;
            let (game, cursor) = (&*self.game, self.cursor);
            ui::draw_board(&mut self.screen, &self.layout, |col, row| {
                point_view(game, cursor, score.as_ref(), Point::new(col, row))
            });
            for p in (0..size * size).map(|i| Point::new(i % size, i / size)) {
                self.drawn[index(p)] = Some(point_view(game, cursor, score.as_ref(), p));
            }
            self.panel = [const { None }; MAX_PANEL_LINES];
        } else {
            for p in (0..size * size).map(|i| Point::new(i % size, i / size)) {
                let view = point_view(&self.game, self.cursor, score.as_ref(), p);
                if self.drawn[index(p)] != Some(view) {
                    ui::draw_point(
                        &mut self.screen,
                        &self.layout,
                        p.col as usize,
                        p.row as usize,
                        view,
                    );
                    self.drawn[index(p)] = Some(view);
                }
            }
        }

        self.render_panel(score.as_ref());
    }

    fn render_panel(&mut self, score: Option<&Score>) {
        let mut lines: [Option<(Line, Color)>; MAX_PANEL_LINES] = [const { None }; MAX_PANEL_LINES];
        let mut put = |i: usize, color: Color, args: core::fmt::Arguments| {
            if i < MAX_PANEL_LINES {
                let mut line = Line::new();
                let _ = line.write_fmt(args);
                lines[i] = Some((line, color));
            }
        };
        let game = &*self.game;
        let size = game.size();

        put(0, ui::TEXT, format_args!("GoOS"));
        let mut komi = Line::new();
        write_half_points(&mut komi, game.komi_half() as i32);
        let versus = if self.opponent == Opponent::Human {
            ""
        } else {
            "   vs computer"
        };
        put(
            1,
            ui::DIM_TEXT,
            format_args!("{size}x{size}   komi {}{versus}", komi.as_str()),
        );
        match game.phase() {
            Phase::Playing if self.opponent == Opponent::Computer(game.to_move()) => put(
                3,
                ui::TEXT,
                format_args!(
                    "{} thinking... {}",
                    name(game.to_move()),
                    self.bot.playouts()
                ),
            ),
            Phase::Playing => put(
                3,
                ui::TEXT,
                format_args!("{} to play", name(game.to_move())),
            ),
            Phase::Scoring => put(3, ui::ACCENT, format_args!("Scoring: mark dead stones")),
            Phase::Finished => put(3, ui::ACCENT, format_args!("Game over")),
        }
        put(4, ui::DIM_TEXT, format_args!("Move {}", game.move_number()));
        put(
            5,
            ui::DIM_TEXT,
            format_args!(
                "Captures  B {}  W {}",
                game.captures(Player::Black),
                game.captures(Player::White)
            ),
        );
        let (mut black, mut white) = (TextBuf::<12>::new(), TextBuf::<12>::new());
        write_clock(&mut black, self.clocks[0]);
        write_clock(&mut white, self.clocks[1]);
        put(
            6,
            ui::DIM_TEXT,
            format_args!("Time  B {}  W {}", black.as_str(), white.as_str()),
        );
        if let Some(n) = &self.notice {
            put(7, n.color, format_args!("{}", n.text.as_str()));
        } else if let (Phase::Playing, true, Some((who, Move::Pass))) =
            (game.phase(), game.passes() > 0, game.last_move())
        {
            put(7, ui::ACCENT, format_args!("{} passed", name(who)));
        }
        if let Some(s) = score {
            put(9, ui::TEXT, format_args!("Black  {}", s.black));
            put(
                10,
                ui::TEXT,
                format_args!("White  {} + {} komi", s.white, komi.as_str()),
            );
            let mut result = Line::new();
            write_result(&mut result, s, game.phase() == Phase::Finished);
            put(11, ui::ACCENT, format_args!("{}", result.as_str()));
        }

        let help: &[&str] = match game.phase() {
            Phase::Playing => &[
                "Arrows/WASD  move",
                "Space/Enter  place",
                "P pass   U undo",
                "N new game  G save",
            ],
            Phase::Scoring => &[
                "Space  mark dead group",
                "Enter  confirm score",
                "Esc  resume play",
                "U undo   N new game",
            ],
            Phase::Finished => &["U  back to scoring", "N new game  G save"],
        };
        // Help goes at the bottom of the panel, below the score lines; it's
        // left out if the panel is too short.
        const FIRST_FREE_LINE: usize = 13;
        let rows = self.layout.panel_lines().min(MAX_PANEL_LINES);
        if rows >= FIRST_FREE_LINE + help.len() {
            for (i, text) in help.iter().enumerate() {
                put(rows - help.len() + i, ui::DIM_TEXT, format_args!("{text}"));
            }
        }

        for (i, line) in lines.iter().enumerate().take(rows) {
            let changed = match (line, &self.panel[i]) {
                (Some((a, ca)), Some((b, cb))) => a.as_str() != b.as_str() || ca != cb,
                (None, None) => false,
                _ => true,
            };
            if changed {
                let (text, color) = line
                    .as_ref()
                    .map_or(("", ui::TEXT), |(t, c)| (t.as_str(), *c));
                let font: Font = if i == 0 {
                    self.layout.title_font
                } else {
                    self.layout.text_font
                };
                ui::panel_line(&mut self.screen, &self.layout, i, text, font, color);
                self.panel[i] = line.as_ref().map(|(t, c)| (copy_line(t), *c));
            }
        }
    }
}

fn point_view(game: &Game, cursor: Point, score: Option<&Score>, p: Point) -> PointView {
    let stone = match game.board().get(p) {
        Cell::Stone(c) => Some(c),
        Cell::Empty => None,
    };
    let playing = game.phase() == Phase::Playing;
    let is_cursor = p == cursor && game.phase() != Phase::Finished;
    PointView {
        stone,
        dead: stone.is_some() && game.is_dead(p),
        territory: score.and_then(|s| s.owner(p)),
        last_move: matches!(game.last_move(), Some((_, Move::Play(q))) if q == p),
        cursor: is_cursor,
        ghost: if is_cursor && playing && stone.is_none() {
            Some(game.to_move())
        } else {
            None
        },
    }
}

fn index(p: Point) -> usize {
    p.row as usize * MAX_SIZE + p.col as usize
}

fn name(player: Player) -> &'static str {
    match player {
        Player::Black => "Black",
        Player::White => "White",
    }
}

/// Go coordinate such as "D4" (columns skip I, row 1 is the bottom).
fn coordinate(p: Point, size: usize) -> TextBuf<4> {
    let mut text = TextBuf::new();
    let _ = write!(
        text,
        "{}{}",
        COLUMNS[p.col as usize] as char,
        size - p.row as usize
    );
    text
}

/// Writes a half-point count such as 15 as "7.5".
fn write_half_points(out: &mut impl Write, half: i32) {
    let _ = if half % 2 == 0 {
        write!(out, "{}", half / 2)
    } else {
        write!(out, "{}.5", half / 2)
    };
}

/// Writes a tick count as "m:ss" (or "h:mm:ss").
fn write_clock(out: &mut impl Write, ticks: u64) {
    let secs = ticks / TICK_HZ;
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    let _ = if h > 0 {
        write!(out, "{h}:{m:02}:{s:02}")
    } else {
        write!(out, "{m}:{s:02}")
    };
}

fn write_result(out: &mut Line, score: &Score, finished: bool) {
    let margin = score.margin_half();
    let Some(winner) = score.winner() else {
        let _ = out.write_str("Draw");
        return;
    };
    let _ = write!(
        out,
        "{} {} by ",
        name(winner),
        if finished { "wins" } else { "leads" }
    );
    write_half_points(out, margin.abs());
}

fn copy_line(line: &Line) -> Line {
    let mut copy = Line::new();
    let _ = copy.write_str(line.as_str());
    copy
}
