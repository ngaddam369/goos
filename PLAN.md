# GoOS — a bare-metal Go (the board game) kernel in Rust

## Goal

Build a small x86_64 kernel in Rust that boots directly into a playable game of Go, in the spirit of jdah's Tetris OS. No host operating system: the kernel sets up interrupts, draws to a framebuffer, reads the keyboard, and runs the game.

The first target is **two players at one keyboard (hotseat)** on a 9×9, 13×13, or 19×19 board, with full rules and scoring. A computer opponent is a stretch goal.

It must build and run with **one command** in QEMU on the developer's laptop.

---

## Ground rules for Claude Code

- Work milestone by milestone (below). Finish and verify one milestone before starting the next.
- After each milestone: make sure it builds, run it in QEMU, run the tests, and make a git commit.
- Keep the Go rules **completely separate from hardware code** so they can be unit-tested on the host with `cargo test`.
- Crate APIs in the OS ecosystem change between versions (especially `bootloader`). Check the current docs and versions on crates.io / docs.rs before writing code against them; don't rely on memory.
- Prefer simple, readable code over cleverness. No heap allocator unless a milestone really needs one; fixed-size arrays are enough.
- If a design choice in this plan turns out to be impractical, explain why and propose an alternative before deviating.

---

## Tech stack

| Purpose | Choice |
|---|---|
| Language | Rust **nightly** (`#![no_std]`, `#![no_main]`, `abi_x86_interrupt`) |
| Bootloader | `bootloader` crate (0.11-style: kernel crate + a runner/builder crate that produces a disk image) with a linear framebuffer |
| CPU structures | `x86_64` crate (GDT, IDT, port I/O) |
| Interrupt controller | `pic8259` |
| Keyboard decoding | `pc-keyboard` (handles extended scancodes for arrow keys) |
| Locking | `spin` |
| Serial debug log | `uart_16550` |
| Text rendering | `noto-sans-mono-bitmap` or `font8x8` |
| Emulator | `qemu-system-x86_64` |

Reference: Philipp Oppermann, *Writing an OS in Rust* (os.phil-opp.com). Follow its approach for the GDT, IDT, PIC and keyboard setup.

Required toolchain setup (document it in the README):

```
rustup toolchain install nightly
rustup component add rust-src llvm-tools-preview --toolchain nightly
```

Pin nightly with a `rust-toolchain.toml` in the repo root.

---

## Project layout

```
goos/
├── Cargo.toml              # workspace
├── rust-toolchain.toml     # pinned nightly
├── README.md               # how to build, run, and play
├── go-core/                # pure Go rules, #![no_std], no hardware, fully unit-tested
│   └── src/
│       ├── lib.rs
│       ├── board.rs        # board state, play, captures, liberties
│       ├── zobrist.rs      # hashing for superko
│       ├── game.rs         # turns, passes, history, undo, phases
│       └── score.rs        # area scoring, dead-stone marking
├── kernel/                 # the bare-metal kernel
│   └── src/
│       ├── main.rs         # entry point, init, main loop
│       ├── serial.rs       # serial_println! for debugging
│       ├── gdt.rs          # GDT + TSS (double-fault stack)
│       ├── interrupts.rs   # IDT, PIC, timer + keyboard handlers
│       ├── input.rs        # key events queue → game actions
│       ├── framebuffer.rs  # pixel drawing primitives, handles RGB/BGR
│       ├── font.rs         # text drawing
│       └── ui.rs           # draws board, stones, cursor, status bar
└── runner/                 # host program: builds the disk image and launches QEMU
```

The single command to run everything should be something like `cargo run --release` from the workspace root (via the runner crate). Use whatever the current `bootloader` docs recommend to achieve that.

---

## Milestones

### M1 — Boot to a colored screen
- Workspace, toolchain file, kernel and runner crates set up.
- Kernel boots in QEMU, fills the framebuffer with a solid wood-like color.
- `serial_println!("GoOS booted")` appears in the terminal (run QEMU with `-serial stdio`).
- Panic handler prints the panic message to serial and halts.

**Done when:** `cargo run` opens QEMU showing the color, and the boot message is in the terminal.

### M2 — Interrupts
- GDT with a TSS and a separate double-fault stack.
- IDT with handlers for breakpoint, double fault, page fault.
- PIC remapped to vectors 32–47; timer (IRQ0) and keyboard (IRQ1) enabled.
- Timer increments a global tick counter; main loop uses `hlt` while idle.
- Keyboard handler decodes scancodes with `pc-keyboard` and pushes key events into a small lock-free or spinlock-protected ring buffer. The handler must not draw anything itself.

**Done when:** pressing keys logs them to serial, and the tick counter visibly advances (log once per second).

### M3 — Drawing the board
- Framebuffer primitives: `put_pixel`, `fill_rect`, `hline`, `vline`, `fill_circle`. Respect the framebuffer's pixel format (RGB vs BGR) and stride.
- Text rendering for a status bar.
- Draw a board scaled to fit the screen: grid lines, star points (hoshi) at standard positions for 9/13/19, coordinate labels (A–T skipping I, 1–19).
- Draw black and white stones as filled circles with a thin outline.
- **Avoid flicker:** redraw only what changed (a cell and its neighbors, the status bar), rather than clearing the whole screen every frame.

**Done when:** a 19×19 board with a few hard-coded stones looks correct and clean.

### M4 — Go rules in `go-core` (host-tested)
This crate must be `#![no_std]` with no allocation, and must compile and test on the host.

- `Board` with a fixed `[Cell; 19 * 19]` array plus a `size` field (9, 13, or 19).
- `play(point, color) -> Result<MoveInfo, IllegalMove>`:
  - Reject occupied points.
  - Remove opponent groups left with zero liberties (flood fill).
  - Reject suicide (a move that leaves its own group with no liberties after captures).
  - Reject **positional superko**: keep a history of Zobrist hashes (fixed-size array, e.g. 1024 entries) and reject any move that recreates an earlier position.
- Zobrist table generated at compile time with a `const fn` splitmix64 PRNG.
- Capture counts per player.
- `Game` type: whose turn it is, pass, two consecutive passes end play, undo (keep a history of board states or moves), new game with chosen size.
- Scoring (area / Chinese rules):
  - Stones on board + empty regions bordered only by that color.
  - Komi stored as an integer in half-points (default 7.5 → `15`) to avoid floats.
  - A dead-stone marking phase: toggling a point marks or unmarks its whole group as dead before final scoring.

**Unit tests required** (run with `cargo test -p go-core`):
- single-stone capture, multi-stone capture, capture at edge and corner
- suicide rejected; a move that looks like suicide but captures is allowed
- simple ko rejected immediately, allowed after a move elsewhere
- a superko case
- scoring on a small finished 9×9 position with a known result
- undo restores board, captures and turn

**Done when:** all tests pass on the host.

### M5 — Playable hotseat game
Wire `go-core` into the kernel.

Controls:
| Key | Action |
|---|---|
| Arrow keys / WASD | move cursor |
| Space / Enter | place stone (or toggle dead group during scoring) |
| P | pass |
| U | undo |
| N | new game (cycle 9 → 13 → 19 on a start screen) |
| Esc | during scoring, return to play |

UI:
- Visible cursor (a highlighted square or small marker at the intersection).
- Marker on the last move played.
- Status bar: whose turn, captures for each side, move number, "Black passed" / "White passed" messages.
- Illegal moves show a brief message (e.g. "Ko", "Suicide", "Occupied") for about 1.5 seconds using the tick counter.
- After two passes, enter scoring mode: dead stones drawn faded, territory shown with small squares, live score shown, Enter to confirm final result.

**Done when:** a complete game can be played from an empty board to a final score in QEMU.

### M6 — Polish
- Start screen with title and board size choice.
- Game clocks per player using the PIT (shown in the status bar). Optional byo-yomi.
- PC speaker "click" when a stone is placed (PIT channel 2 + port 0x61). Make sure QEMU is launched with an audio device option if needed; it must not break if sound is unavailable.
- README with screenshots, controls, and build instructions.
- Test the image on real hardware from a USB stick if possible (optional; document how).

---

## Stretch goals (only after M6)

1. **Computer opponent** on 9×9:
   - Start with a random legal move that doesn't fill its own eyes.
   - Then Monte Carlo: for each candidate move, run N random playouts and pick the best win rate.
   - Then MCTS with UCB using integer or fixed-point math.
   - Seed the PRNG from `rdtsc`.
   - Run thinking in the main loop in small slices so the UI stays responsive.
   - Add `go-core` tests for the playout logic.
2. PS/2 mouse support (IRQ12) to click intersections.
3. Save/load a game as SGF text over serial.

---

## Debugging notes

- Always run QEMU with `-serial stdio` so `serial_println!` output shows up in the terminal.
- For triple faults / reboot loops, add `-d int -no-reboot -no-shutdown` to see which exception happened.
- A double-fault handler on its own IST stack (set up in M2) prevents most silent reboots from stack overflows.
- Keep interrupt handlers tiny: read the scancode, push it to the queue, send EOI. All drawing happens in the main loop.
- If the screen colors look swapped, the framebuffer is BGR, not RGB.

---

## Definition of done (core project)

- `cargo run --release` builds a bootable image and launches it in QEMU.
- `cargo test -p go-core` passes.
- A full hotseat game of Go can be played on 9×9, 13×13 and 19×19 with captures, ko/superko, passing, dead-stone marking and area scoring with komi.
- README explains setup, controls, and how it works.
