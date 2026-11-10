//! The rules of reversi (Othello) on the 8×8 board: positions, moves, notation, and a game with its history.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Color {
    Black,
    White,
}

impl Color {
    pub fn opposite(self) -> Color {
        match self {
            Color::Black => Color::White,
            Color::White => Color::Black,
        }
    }
}

/// The squares: 0 is a1, 1 b1 ... 7 h1, 8 a2 ... 63 h8. Row 1 is at the top of a diagram, as on a printed board.
pub type Square = i32;
pub const SQUARE_COUNT: i32 = 64;
/// The move of a side that cannot place a disc.
pub const PASS: Square = -1;

pub const fn file_of(square: Square) -> i32 {
    square & 7
}

pub const fn row_of(square: Square) -> i32 {
    square >> 3
}

/// The square of a file and a row, or -1 off the board.
pub const fn square_at(file: i32, row: i32) -> Square {
    if file >= 0 && file < 8 && row >= 0 && row < 8 { row * 8 + file } else { -1 }
}

pub const fn bit(square: Square) -> u64 {
    1 << square
}

/// The squares of a mask, from a1 on.
pub fn squares(mut bits: u64) -> impl Iterator<Item = Square> {
    std::iter::from_fn(move || {
        (bits != 0).then(|| {
            let square = bits.trailing_zeros() as Square;
            bits &= bits - 1;
            square
        })
    })
}

/// "f5"; a pass is "pass".
pub fn square_name(square: Square) -> String {
    if !(0..SQUARE_COUNT).contains(&square) {
        return "pass".to_owned();
    }
    format!("{}{}", char::from(b'a' + file_of(square) as u8), char::from(b'1' + row_of(square) as u8))
}

fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\r' | b'\n')
}

/// The opponent's discs that a run may pass through sideways: away from the side edges, so that nothing
/// wraps round from one side of the board to the other.
const INNER: u64 = 0x7E7E_7E7E_7E7E_7E7E;

/// The opponent's discs in runs from the player's, along a direction and its opposite, one to six long.
fn runs<const STEP: u32>(from: u64, inner: u64) -> u64 {
    let (mut forward, mut backward) = (inner & (from << STEP), inner & (from >> STEP));
    forward |= inner & (forward << STEP);
    backward |= inner & (backward >> STEP);
    let (pairs_forward, pairs_backward) = (inner & (inner << STEP), inner & (inner >> STEP));
    forward |= pairs_forward & (forward << (2 * STEP));
    backward |= pairs_backward & (backward >> (2 * STEP));
    forward |= pairs_forward & (forward << (2 * STEP));
    backward |= pairs_backward & (backward >> (2 * STEP));
    (forward << STEP) | (backward >> STEP)
}

/// The opponent's discs between a new disc and the player's, along a direction and its opposite.
fn line<const STEP: u32>(start: u64, player: u64, inner: u64) -> u64 {
    let (mut forward, mut backward) = (inner & (start << STEP), inner & (start >> STEP));
    for _ in 0..5 {
        forward |= inner & (forward << STEP);
        backward |= inner & (backward >> STEP);
    }
    let mut flips = 0;
    if (forward << STEP) & player != 0 {
        flips |= forward;
    }
    if (backward >> STEP) & player != 0 {
        flips |= backward;
    }
    flips
}

/// The empty squares where `player` can place a disc.
pub fn moves_of(player: u64, opponent: u64) -> u64 {
    let inner = opponent & INNER;
    (runs::<1>(player, inner) | runs::<8>(player, opponent) | runs::<7>(player, inner) | runs::<9>(player, inner)) & !(player | opponent)
}

/// The opponent's discs that a disc placed on `square` turns over.
pub fn flips_of(square: Square, player: u64, opponent: u64) -> u64 {
    let (start, inner) = (bit(square), opponent & INNER);
    line::<1>(start, player, inner) | line::<8>(start, player, opponent) | line::<7>(start, player, inner) | line::<9>(start, player, inner)
}

const fn mix(mut x: u64) -> u64 {
    x ^= x >> 33;
    x = x.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    x ^= x >> 33;
    x = x.wrapping_mul(0xC4CE_B9FE_1A85_EC53);
    x ^= x >> 33;
    x
}

/// A key for a table of positions, from the discs of the side to move and of the other side.
pub const fn hash(player: u64, opponent: u64) -> u64 {
    mix(player) ^ mix(opponent ^ 0x9E37_79B9_7F4A_7C15)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Position {
    black: u64,
    white: u64,
    side: Color,
}

impl Position {
    /// d4 and e5 white, d5 and e4 black; Black moves first.
    pub fn start() -> Position {
        Position { white: bit(square_at(3, 3)) | bit(square_at(4, 4)), black: bit(square_at(4, 3)) | bit(square_at(3, 4)), side: Color::Black }
    }

    /// 64 squares from a1 to h8, X (or *) for Black, O for White, - (or .) for empty, then X or O for the side to move.
    pub fn from_text(text: &str) -> Result<Position, String> {
        let mut position = Position { black: 0, white: 0, side: Color::Black };
        let mut characters = text.bytes().filter(|c| !is_space(*c));
        for square in 0..SQUARE_COUNT {
            match characters.next() {
                Some(b'X' | b'x' | b'*') => position.black |= bit(square),
                Some(b'O' | b'o') => position.white |= bit(square),
                Some(b'-' | b'.' | b'_') => {}
                Some(c) => return Err(format!("unexpected '{}' on the board", char::from(c))),
                None => return Err("the board has fewer than 64 squares".to_owned()),
            }
        }
        match characters.next() {
            Some(b'O' | b'o' | b'W' | b'w') => position.side = Color::White,
            Some(b'X' | b'x' | b'*' | b'B' | b'b') | None => {}
            Some(_) => return Err("the side to move is not X or O".to_owned()),
        }
        Ok(position)
    }

    pub fn to_text(self) -> String {
        let mut text: String = (0..SQUARE_COUNT)
            .map(|square| match self.at(square) {
                Some(Color::Black) => 'X',
                Some(Color::White) => 'O',
                None => '-',
            })
            .collect();
        text.push(' ');
        text.push(if self.side == Color::Black { 'X' } else { 'O' });
        text
    }

    pub fn side_to_move(&self) -> Color {
        self.side
    }

    pub fn discs(&self, color: Color) -> u64 {
        match color {
            Color::Black => self.black,
            Color::White => self.white,
        }
    }

    pub fn player(&self) -> u64 {
        self.discs(self.side)
    }

    pub fn opponent(&self) -> u64 {
        self.discs(self.side.opposite())
    }

    pub fn empty(&self) -> u64 {
        !(self.black | self.white)
    }

    pub fn count(&self, color: Color) -> i32 {
        self.discs(color).count_ones() as i32
    }

    pub fn empty_count(&self) -> i32 {
        self.empty().count_ones() as i32
    }

    pub fn at(&self, square: Square) -> Option<Color> {
        if self.black & bit(square) != 0 {
            Some(Color::Black)
        } else if self.white & bit(square) != 0 {
            Some(Color::White)
        } else {
            None
        }
    }

    /// Where the side to move can place a disc.
    pub fn moves(&self) -> u64 {
        moves_of(self.player(), self.opponent())
    }

    pub fn is_legal(&self, square: Square) -> bool {
        (0..SQUARE_COUNT).contains(&square) && self.moves() & bit(square) != 0
    }

    /// What a disc of the side to move turns over there.
    pub fn flips(&self, square: Square) -> u64 {
        flips_of(square, self.player(), self.opponent())
    }

    /// The side to move has nowhere to go, but the other side has.
    pub fn must_pass(&self) -> bool {
        self.moves() == 0 && moves_of(self.opponent(), self.player()) != 0
    }

    /// Neither side can move: the game is over.
    pub fn is_over(&self) -> bool {
        self.moves() == 0 && moves_of(self.opponent(), self.player()) == 0
    }

    /// The position after a legal move, or a pass.
    pub fn after(&self, square: Square) -> Position {
        let mut next = *self;
        if square >= 0 {
            let flips = self.flips(square);
            let (own, other) = match self.side {
                Color::Black => (&mut next.black, &mut next.white),
                Color::White => (&mut next.white, &mut next.black),
            };
            *own |= flips | bit(square);
            *other &= !flips;
        }
        next.side = self.side.opposite();
        next
    }

    pub fn key(&self) -> u64 {
        hash(self.black, self.white) ^ if self.side == Color::White { 0x2545_F491_4F6C_DD1D } else { 0 }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GameResult {
    #[default]
    Ongoing,
    BlackWins,
    WhiteWins,
    Draw,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct GameStatus {
    pub result: GameResult,
    // The discs on the board.
    pub black: i32,
    pub white: i32,
}

impl GameStatus {
    pub fn over(&self) -> bool {
        self.result != GameResult::Ongoing
    }
}

#[derive(Clone, Copy, Debug)]
pub struct MoveRecord {
    pub square: Square,
    pub side: Color,
    pub flipped: u64, // the discs turned over
}

impl MoveRecord {
    pub fn is_pass(&self) -> bool {
        self.square == PASS
    }
}

/// A game: the starting position, the moves played, and the moves taken back that can be replayed.
/// A side left without a move passes at once; the pass is kept as a move of its own.
#[derive(Clone)]
pub struct Game {
    positions: Vec<Position>,
    moves: Vec<MoveRecord>,
    redo: Vec<MoveRecord>, // the next to replay at the end
    status: GameStatus,
}

impl Game {
    pub fn new() -> Game {
        Game::from_position(Position::start())
    }

    pub fn from_position(start: Position) -> Game {
        let start = if start.must_pass() { start.after(PASS) } else { start };
        let mut game = Game { positions: vec![start], moves: Vec::new(), redo: Vec::new(), status: GameStatus::default() };
        game.update_status();
        game
    }

    /// A transcript ("f5d6c3d3"), a position, or a position and then a transcript.
    pub fn parse(text: &str) -> Result<Game, String> {
        let bytes = text.as_bytes();
        let mut game = Game::new();

        // A board first, if the text begins with 64 squares.
        let begin = bytes.iter().position(|c| !is_space(*c)).unwrap_or(bytes.len());
        let (mut squares, mut end) = (0, begin);
        while end < bytes.len() && squares < SQUARE_COUNT {
            let c = bytes[end];
            if !is_space(c) {
                if !b"Xx*Oo-._".contains(&c) {
                    break;
                }
                squares += 1;
            }
            end += 1;
        }
        let mut rest = bytes;
        let has_position = squares == SQUARE_COUNT;
        if has_position {
            // The side to move may follow.
            while end < bytes.len() && is_space(bytes[end]) {
                end += 1;
            }
            let mut side_end = end;
            if side_end < bytes.len() && b"XxOoBbWw*".contains(&bytes[side_end]) && bytes.get(side_end + 1).is_none_or(|next| !next.is_ascii_digit()) {
                side_end += 1;
            }
            game = Game::from_position(Position::from_text(&text[begin..side_end])?);
            rest = &bytes[side_end..];
        }

        // Then the moves: every letter a-h followed by a digit 1-8; passes are left out or skipped.
        let mut count = 0;
        let mut i = 0;
        while i + 1 < rest.len() {
            let (letter, digit) = (rest[i].to_ascii_lowercase(), rest[i + 1]);
            // Part of a longer word, such as a name, is not a move.
            let in_word = i > 0 && rest[i - 1].is_ascii_alphabetic();
            if (b'a'..=b'h').contains(&letter) && (b'1'..=b'8').contains(&digit) && !in_word {
                let square = square_at(i32::from(letter - b'a'), i32::from(digit - b'1'));
                count += 1;
                if !game.play(square) {
                    return Err(format!("move {count} ({}) is not legal", square_name(square)));
                }
                i += 1;
            }
            i += 1;
        }

        if count == 0 && !has_position {
            return Err("no moves or position".to_owned());
        }
        Ok(game)
    }

    /// The starting position and the moves, for saving.
    pub fn serialize(&self) -> String {
        format!("{}|{}", self.start().to_text(), self.transcript())
    }

    pub fn deserialize(text: &str) -> Option<Game> {
        let (start, moves) = text.split_once('|')?;
        let start = Position::from_text(start).ok()?;
        Game::parse(&format!("{} {moves}", start.to_text())).ok()
    }

    /// The moves as Othello transcripts are written, without the passes: "f5d6c3d3".
    pub fn transcript(&self) -> String {
        self.moves.iter().filter(|record| !record.is_pass()).map(|record| square_name(record.square)).collect()
    }

    /// For the clipboard: the transcript, after the starting position when it is not the usual one.
    pub fn to_text(&self) -> String {
        if *self.start() == Position::start() {
            return self.transcript();
        }
        format!("{}\r\n{}", self.start().to_text(), self.transcript())
    }

    pub fn current(&self) -> &Position {
        self.positions.last().expect("a game has its starting position")
    }

    pub fn start(&self) -> &Position {
        &self.positions[0]
    }

    pub fn moves(&self) -> &[MoveRecord] {
        &self.moves
    }

    pub fn ply(&self) -> usize {
        self.moves.len()
    }

    pub fn status(&self) -> GameStatus {
        self.status
    }

    /// Places a disc on a legal square; a pass that it forces on the other side follows.
    pub fn play(&mut self, square: Square) -> bool {
        if self.status.over() || !self.current().is_legal(square) {
            return false;
        }
        self.push(square);
        if self.current().must_pass() {
            self.push(PASS);
        }
        self.redo.clear();
        self.update_status();
        true
    }

    pub fn can_undo(&self) -> bool {
        !self.moves.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Takes back the last disc placed, with the pass after it.
    pub fn undo(&mut self) -> bool {
        let mut undone = false;
        while let Some(record) = self.moves.pop() {
            self.positions.pop();
            self.redo.push(record);
            undone = true;
            if !record.is_pass() {
                break;
            }
        }
        self.update_status();
        undone
    }

    pub fn redo(&mut self) -> bool {
        // The disc, then the pass that came after it.
        let Some(record) = self.redo.pop() else { return false };
        self.push(record.square);
        while self.redo.last().is_some_and(MoveRecord::is_pass) {
            self.redo.pop();
            self.push(PASS);
        }
        self.update_status();
        true
    }

    fn push(&mut self, square: Square) {
        let position = *self.current();
        let flipped = if square >= 0 { position.flips(square) } else { 0 };
        self.moves.push(MoveRecord { square, side: position.side_to_move(), flipped });
        self.positions.push(position.after(square));
    }

    fn update_status(&mut self) {
        let position = *self.current();
        let (black, white) = (position.count(Color::Black), position.count(Color::White));
        let result = if !position.is_over() {
            GameResult::Ongoing
        } else {
            match black.cmp(&white) {
                std::cmp::Ordering::Greater => GameResult::BlackWins,
                std::cmp::Ordering::Less => GameResult::WhiteWins,
                std::cmp::Ordering::Equal => GameResult::Draw,
            }
        };
        self.status = GameStatus { result, black, white };
    }
}
