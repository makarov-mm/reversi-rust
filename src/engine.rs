//! The computer player: an alpha-beta searcher with iterative deepening, principal variation search and a
//! transposition table in the middle game, and an exact solver for the end, which orders its moves to leave
//! the opponent the fewest replies.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::reversi::{PASS, Position, SQUARE_COUNT, Square, bit, file_of, flips_of, hash, moves_of, row_of, square_at, squares};

#[derive(Clone, Copy)]
pub struct SearchLimits {
    pub max_depth: i32,
    pub milliseconds: u64,  // iterative deepening stops when this runs out
    pub exact_empties: i32, // from this many empty squares the game is played out to the end, exactly
    pub random_margin: i32, // a random move among those this close to the best, for the weaker levels
    pub seed: u64,
}

#[derive(Clone, Copy)]
pub struct SearchResult {
    pub best: Square,
    pub score: i32, // for the side to move: the evaluation, or the final difference in discs when solved
    pub depth: i32,
    pub solved: bool, // the score is exact
    pub nodes: u64,
}

impl Default for SearchResult {
    fn default() -> SearchResult {
        SearchResult { best: PASS, score: 0, depth: 0, solved: false, nodes: 0 }
    }
}

const INFINITY: i32 = 1 << 20;
/// A won game, and then the discs it is won by.
const WIN: i32 = 30000;

fn is_win_score(score: i32) -> bool {
    !(-WIN + 100..=WIN - 100).contains(&score)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Bound {
    Upper,
    Lower,
    Exact,
}

const NOT_FILE_A: u64 = 0xFEFE_FEFE_FEFE_FEFE;
const NOT_FILE_H: u64 = 0x7F7F_7F7F_7F7F_7F7F;
const FILE_A: u64 = 0x0101_0101_0101_0101;
const FILE_H: u64 = 0x8080_8080_8080_8080;
const ROW_1: u64 = 0x0000_0000_0000_00FF;
const ROW_8: u64 = 0xFF00_0000_0000_0000;
const EDGES: u64 = FILE_A | FILE_H | ROW_1 | ROW_8;
const CORNERS: u64 = 0x8100_0000_0000_0081;

/// A corner, the square diagonally next to it (the X-square) and the two beside it on the edges (the C-squares):
/// taking those while the corner is empty usually hands the corner over.
struct CornerArea {
    corner: u64,
    x: u64,
    c: u64,
}

const CORNER_AREAS: [CornerArea; 4] = [
    CornerArea { corner: bit(0), x: bit(9), c: bit(1) | bit(8) },
    CornerArea { corner: bit(7), x: bit(14), c: bit(6) | bit(15) },
    CornerArea { corner: bit(56), x: bit(49), c: bit(57) | bit(48) },
    CornerArea { corner: bit(63), x: bit(54), c: bit(62) | bit(55) },
];

/// The worth of holding each square, for the evaluation and for ordering the moves.
#[rustfmt::skip]
const SQUARE_VALUE: [i32; 64] = [
    20, -3, 11,  8,  8, 11, -3, 20,
    -3, -7, -4,  1,  1, -4, -7, -3,
    11, -4,  2,  2,  2,  2, -4, 11,
     8,  1,  2, -3, -3,  2,  1,  8,
     8,  1,  2, -3, -3,  2,  1,  8,
    11, -4,  2,  2,  2,  2, -4, 11,
    -3, -7, -4,  1,  1, -4, -7, -3,
    20, -3, 11,  8,  8, 11, -3, 20,
];

/// The squares of each value, so that the position can be scored a few masks at a time.
const VALUE_MASKS: [(i32, u64); 8] = {
    let mut masks = [(0, 0); 8];
    let mut count = 0;
    let mut square = 0;
    while square < 64 {
        let mut i = 0;
        while i < count && masks[i].0 != SQUARE_VALUE[square] {
            i += 1;
        }
        if i == count {
            masks[count].0 = SQUARE_VALUE[square];
            count += 1;
        }
        masks[i].1 |= 1 << square;
        square += 1;
    }
    masks
};

/// The diagonals in both directions, for finding the full ones.
const DIAGONALS: ([u64; 15], [u64; 15]) = {
    let (mut diagonals, mut anti_diagonals) = ([0; 15], [0; 15]);
    let mut square = 0;
    while square < 64 {
        diagonals[(file_of(square) - row_of(square) + 7) as usize] |= bit(square);
        anti_diagonals[(file_of(square) + row_of(square)) as usize] |= bit(square);
        square += 1;
    }
    (diagonals, anti_diagonals)
};

/// The squares next to each square.
const NEIGHBOUR_MASKS: [u64; 64] = {
    let mut masks = [0; 64];
    let mut square = 0;
    while square < 64 {
        let mut dy = -1;
        while dy <= 1 {
            let mut dx = -1;
            while dx <= 1 {
                let neighbour = square_at(file_of(square) + dx, row_of(square) + dy);
                if (dx != 0 || dy != 0) && neighbour >= 0 {
                    masks[square as usize] |= bit(neighbour);
                }
                dx += 1;
            }
            dy += 1;
        }
        square += 1;
    }
    masks
};

/// The quarters of the board: at the very end, a move into a quarter with an odd number of empty squares
/// tends to get the last move there.
const QUARTERS: [u64; 4] = [0x0000_0000_0F0F_0F0F, 0x0000_0000_F0F0_F0F0, 0x0F0F_0F0F_0000_0000, 0xF0F0_F0F0_0000_0000];

/// The empty squares in the quarters that have an odd number of them.
fn odd_quarters(empty: u64) -> u64 {
    QUARTERS.iter().filter(|quarter| (empty & **quarter).count_ones() & 1 != 0).fold(0, |odd, quarter| odd | quarter)
}

/// The squares next to any of the given ones.
fn neighbours(bits: u64) -> u64 {
    let sideways = ((bits << 1) & NOT_FILE_A) | ((bits >> 1) & NOT_FILE_H);
    let row = bits | sideways;
    sideways | (row << 8) | (row >> 8)
}

/// The discs that can never be turned over: along each of the four lines through such a disc, the line is
/// full, or the disc has the edge or another such disc of its color next to it.
fn stable_discs(own: u64, occupied: u64) -> u64 {
    if own & CORNERS == 0 {
        return 0; // there are none without a corner, but in full lines along the edges, which are rare
    }
    let full_rows = (0..8).map(|row| 0xFF << (8 * row)).filter(|row| occupied & row == *row).fold(0, |rows, row| rows | row);
    let mut columns = occupied & (occupied >> 8);
    columns &= columns >> 16;
    columns &= columns >> 32;
    let full_columns = (columns & 0xFF) * FILE_A;
    let full = |lines: &[u64; 15]| lines.iter().filter(|line| occupied & **line == **line).fold(0, |full, line| full | line);
    let (full_diagonals, full_anti_diagonals) = (full(&DIAGONALS.0), full(&DIAGONALS.1));

    let mut stable = 0;
    loop {
        let across = full_rows | FILE_A | FILE_H | ((stable << 1) & NOT_FILE_A) | ((stable >> 1) & NOT_FILE_H);
        let down = full_columns | ROW_1 | ROW_8 | (stable << 8) | (stable >> 8);
        let diagonal = full_diagonals | EDGES | ((stable << 9) & NOT_FILE_A) | ((stable >> 9) & NOT_FILE_H);
        let anti_diagonal = full_anti_diagonals | EDGES | ((stable << 7) & NOT_FILE_H) | ((stable >> 7) & NOT_FILE_A);
        let next = own & across & down & diagonal & anti_diagonal;
        if next == stable {
            return stable;
        }
        stable = next;
    }
}

fn count(bits: u64) -> i32 {
    bits.count_ones() as i32
}

/// A difference as a share of a total, from -100 to 100.
fn share(own: i32, other: i32) -> i32 {
    if own + other > 0 { 100 * (own - other) / (own + other) } else { 0 }
}

// The weights of the evaluation's terms; the sum is divided by eight.
const WEIGHT_MOBILITY: i32 = 237; // times the difference in moves as a share of all moves, -100 to 100
const WEIGHT_FRONTIER: i32 = 166; // the same for the discs next to empty squares, which count against
const WEIGHT_CORNER: i32 = 8900; // for each corner
const WEIGHT_X_SQUARE: i32 = 3183; // against each disc diagonally next to an empty corner
const WEIGHT_C_SQUARE: i32 = 1415; // against each disc beside an empty corner on the edge
const WEIGHT_STABLE: i32 = 1250; // for each disc that can no longer be turned
const WEIGHT_DISCS: i32 = 15; // times the difference in discs as a share
const WEIGHT_SQUARES: i32 = 10; // times the worth of the squares held

/// Corners, the squares next to them, mobility, frontier, stable discs and position, for the side to move.
fn evaluate(player: u64, opponent: u64, player_moves: u64) -> i32 {
    let empty = !(player | opponent);
    let mut score = 0;

    // Room to move, now and later: the moves each side has, and the discs each has next to empty squares,
    // which give the other side something to play against.
    score += WEIGHT_MOBILITY * share(count(player_moves), count(moves_of(opponent, player)));
    let frontier = neighbours(empty);
    score -= WEIGHT_FRONTIER * share(count(player & frontier), count(opponent & frontier));

    // The corners, and the squares that give them away while they are empty.
    score += WEIGHT_CORNER * (count(player & CORNERS) - count(opponent & CORNERS));
    for area in &CORNER_AREAS {
        if empty & area.corner != 0 {
            score -= WEIGHT_X_SQUARE * (count(player & area.x) - count(opponent & area.x));
            score -= WEIGHT_C_SQUARE * (count(player & area.c) - count(opponent & area.c));
        }
    }

    // Discs that are there for good.
    let occupied = player | opponent;
    score += WEIGHT_STABLE * (count(stable_discs(player, occupied)) - count(stable_discs(opponent, occupied)));

    // The discs themselves, and where they stand.
    score += WEIGHT_DISCS * share(count(player), count(opponent));
    let squares: i32 = VALUE_MASKS.iter().map(|(value, mask)| value * (count(player & mask) - count(opponent & mask))).sum();
    score += WEIGHT_SQUARES * squares;
    (score / 8).clamp(-WIN + 1000, WIN - 1000)
}

fn final_score(player: u64, opponent: u64) -> i32 {
    let difference = count(player) - count(opponent);
    difference + WIN * difference.signum()
}

/// The last empty square, filled by whichever side can: the final difference in discs.
fn last_move(player: u64, opponent: u64) -> i32 {
    let square = (!(player | opponent)).trailing_zeros() as Square;
    let before = 2 * count(player) - 63;
    let flips = flips_of(square, player, opponent);
    if flips != 0 {
        return before + 2 * count(flips) + 1;
    }
    let flips = flips_of(square, opponent, player);
    if flips != 0 {
        return before - 2 * count(flips) - 1;
    }
    before
}

#[derive(Clone, Copy, Default)]
struct Candidate {
    square: Square,
    flips: u64,
    order: i32,
}

/// The moves of a position, to be taken best first.
struct MoveList {
    list: [Candidate; 32],
    count: usize,
}

impl MoveList {
    fn new() -> MoveList {
        MoveList { list: [Candidate::default(); 32], count: 0 }
    }

    fn push(&mut self, candidate: Candidate) {
        self.list[self.count] = candidate;
        self.count += 1;
    }

    /// Brings the best of the moves from `index` on to `index`, and returns it.
    fn pick(&mut self, index: usize) -> Candidate {
        let mut top = index;
        for other in index + 1..self.count {
            if self.list[other].order > self.list[top].order {
                top = other;
            }
        }
        self.list.swap(index, top);
        self.list[index]
    }
}

#[derive(Clone, Copy)]
struct Entry {
    key: u64,
    score: i32,
    depth: i8,
    bound: Option<Bound>,
    best: i8,
    solved: bool, // the score is the final difference in discs, not an evaluation
}

const EMPTY_ENTRY: Entry = Entry { key: 0, score: 0, depth: 0, bound: None, best: -1, solved: false };

#[derive(Clone, Copy)]
struct RootMove {
    square: Square,
    score: i32,
}

/// Seeds scrambled into a stream of random numbers (SplitMix64).
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// From 0 up to 1.
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

pub struct Engine {
    table: Vec<Entry>,
    nodes: u64,
    deadline: Instant,
    stop: Arc<AtomicBool>,
    aborted: bool,
}

impl Engine {
    pub fn new(hash_megabytes: usize) -> Engine {
        let mut entries = 1;
        while entries * 2 * size_of::<Entry>() <= hash_megabytes * 1024 * 1024 {
            entries *= 2;
        }
        Engine { table: vec![EMPTY_ENTRY; entries], nodes: 0, deadline: Instant::now(), stop: Arc::default(), aborted: false }
    }

    pub fn clear(&mut self) {
        self.table.fill(EMPTY_ENTRY);
    }

    fn should_stop(&self) -> bool {
        self.stop.load(Ordering::Relaxed) || Instant::now() >= self.deadline
    }

    fn slot(&self, key: u64) -> usize {
        key as usize & (self.table.len() - 1)
    }

    fn store(&mut self, key: u64, depth: i32, score: i32, bound: Bound, best: Square, solved: bool) {
        let slot = self.slot(key);
        let entry = &mut self.table[slot];
        if entry.key == key && i32::from(entry.depth) > depth && entry.solved == solved && bound != Bound::Exact {
            return;
        }
        *entry = Entry { key, score, depth: depth.clamp(0, 120) as i8, bound: Some(bound), best: best as i8, solved };
    }

    /// Counts a node, and now and then looks at the clock.
    fn tick(&mut self) -> bool {
        self.nodes += 1;
        if self.nodes & 4095 == 0 && self.should_stop() {
            self.aborted = true;
        }
        self.aborted
    }

    fn negamax(&mut self, player: u64, opponent: u64, depth: i32, mut alpha: i32, beta: i32, passed: bool) -> i32 {
        if self.tick() {
            return 0;
        }

        let moves = moves_of(player, opponent);
        if moves == 0 {
            if passed {
                return final_score(player, opponent);
            }
            return -self.negamax(opponent, player, depth, -beta, -alpha, true); // a pass costs no depth
        }
        if depth <= 0 {
            return evaluate(player, opponent, moves);
        }

        let key = hash(player, opponent);
        let entry = self.table[self.slot(key)];
        let mut table_move = -1;
        if entry.key == key && !entry.solved {
            table_move = Square::from(entry.best);
            if i32::from(entry.depth) >= depth {
                let score = entry.score;
                match entry.bound {
                    Some(Bound::Exact) => return score,
                    Some(Bound::Lower) if score >= beta => return score,
                    Some(Bound::Upper) if score <= alpha => return score,
                    _ => {}
                }
            }
        }

        // Best first: the move that was best here before, then those that leave the opponent the fewest replies.
        let mut list = MoveList::new();
        for square in squares(moves) {
            let flips = flips_of(square, player, opponent);
            let mut order = SQUARE_VALUE[square as usize];
            if square == table_move {
                order = INFINITY;
            } else if depth >= 2 {
                order = 8 * order - 64 * count(moves_of(opponent & !flips, player | flips | bit(square)));
            }
            list.push(Candidate { square, flips, order });
        }

        let alpha_before = alpha;
        let (mut best, mut best_move) = (-INFINITY, -1);
        for i in 0..list.count {
            let candidate = list.pick(i);
            let (next_player, next_opponent) = (opponent & !candidate.flips, player | candidate.flips | bit(candidate.square));
            let mut score;
            if i == 0 {
                score = -self.negamax(next_player, next_opponent, depth - 1, -beta, -alpha, false);
            } else {
                score = -self.negamax(next_player, next_opponent, depth - 1, -alpha - 1, -alpha, false);
                if score > alpha && score < beta {
                    score = -self.negamax(next_player, next_opponent, depth - 1, -beta, -alpha, false);
                }
            }
            if self.aborted {
                return 0;
            }
            if score > best {
                best = score;
                best_move = candidate.square;
            }
            if score > alpha {
                alpha = score;
                if alpha >= beta {
                    break;
                }
            }
        }
        let bound = if best >= beta {
            Bound::Lower
        } else if best > alpha_before {
            Bound::Exact
        } else {
            Bound::Upper
        };
        self.store(key, depth, best, bound, best_move, false);
        best
    }

    /// The last few empty squares: no table and no sorting, only the quarters with an odd number of empty squares
    /// first; the empty squares are tried one by one rather than the moves generated.
    fn solve_small(&mut self, player: u64, opponent: u64, mut alpha: i32, beta: i32, empties: i32, passed: bool) -> i32 {
        self.nodes += 1;
        if empties == 1 {
            return last_move(player, opponent);
        }
        let empty = !(player | opponent);
        let odd = odd_quarters(empty);
        let mut best = -INFINITY;
        for group in [empty & odd, empty & !odd] {
            for square in squares(group) {
                if NEIGHBOUR_MASKS[square as usize] & opponent == 0 {
                    continue;
                }
                let flips = flips_of(square, player, opponent);
                if flips == 0 {
                    continue;
                }
                let score = -self.solve_small(opponent & !flips, player | flips | bit(square), -beta, -alpha, empties - 1, false);
                if score > best {
                    best = score;
                    if score > alpha {
                        alpha = score;
                        if alpha >= beta {
                            return best;
                        }
                    }
                }
            }
        }
        if best == -INFINITY {
            // No move: a pass, or the end.
            if passed {
                return count(player) - count(opponent);
            }
            return -self.solve_small(opponent, player, -beta, -alpha, empties, true);
        }
        best
    }

    fn solve(&mut self, player: u64, opponent: u64, mut alpha: i32, mut beta: i32, empties: i32, passed: bool) -> i32 {
        if empties <= 6 {
            return self.solve_small(player, opponent, alpha, beta, empties, passed);
        }
        if self.tick() {
            return 0;
        }

        let moves = moves_of(player, opponent);
        if moves == 0 {
            if passed {
                return count(player) - count(opponent);
            }
            return -self.solve(opponent, player, -beta, -alpha, empties, true);
        }

        let key = hash(player, opponent);
        let entry = self.table[self.slot(key)];
        let mut table_move = -1;
        if entry.key == key && entry.solved {
            table_move = Square::from(entry.best);
            let score = entry.score;
            match entry.bound {
                Some(Bound::Exact) => return score,
                Some(Bound::Lower) if score >= beta => return score,
                Some(Bound::Upper) if score <= alpha => return score,
                Some(Bound::Lower) => alpha = alpha.max(score),
                Some(Bound::Upper) => beta = beta.min(score),
                None => {}
            }
        }

        // Fastest first: the moves that leave the opponent the fewest replies, corners and the odd quarters breaking ties.
        let odd = odd_quarters(!(player | opponent));
        let mut list = MoveList::new();
        for square in squares(moves) {
            let flips = flips_of(square, player, opponent);
            let (next_player, next_opponent) = (opponent & !flips, player | flips | bit(square));
            let replies = moves_of(next_player, next_opponent);
            let order = if square == table_move {
                INFINITY
            } else if empties >= 12 {
                -evaluate(next_player, next_opponent, replies) - 200 * count(replies)
            } else {
                -16 * count(replies) + if bit(square) & CORNERS != 0 { 12 } else { 0 } + if bit(square) & odd != 0 { 3 } else { 0 }
            };
            list.push(Candidate { square, flips, order });
        }

        let alpha_before = alpha;
        let (mut best, mut best_move) = (-INFINITY, -1);
        for i in 0..list.count {
            let candidate = list.pick(i);
            let (next_player, next_opponent) = (opponent & !candidate.flips, player | candidate.flips | bit(candidate.square));
            let mut score;
            if i == 0 {
                score = -self.solve(next_player, next_opponent, -beta, -alpha, empties - 1, false);
            } else {
                score = -self.solve(next_player, next_opponent, -alpha - 1, -alpha, empties - 1, false);
                if score > alpha && score < beta {
                    score = -self.solve(next_player, next_opponent, -beta, -alpha, empties - 1, false);
                }
            }
            if self.aborted {
                return 0;
            }
            if score > best {
                best = score;
                best_move = candidate.square;
            }
            if score > alpha {
                alpha = score;
                if alpha >= beta {
                    break;
                }
            }
        }
        let bound = if best >= beta {
            Bound::Lower
        } else if best > alpha_before {
            Bound::Exact
        } else {
            Bound::Upper
        };
        self.store(key, empties, best, bound, best_move, true);
        best
    }

    /// Deepens the search of the root's moves until the time is half gone; false when it was cut short.
    fn search_middle(
        &mut self,
        root: &Position,
        moves: &mut [RootMove],
        limits: &SearchLimits,
        result: &mut SearchResult,
        start: Instant,
        budget: Duration,
    ) -> bool {
        let (player, opponent) = (root.player(), root.opponent());
        let exact_scores = limits.random_margin > 0;
        let empties = root.empty_count();
        for depth in 1..=limits.max_depth {
            let mut alpha = -INFINITY;
            let (mut iteration_best, mut iteration_score) = (PASS, -INFINITY);
            let mut complete = true;
            for (i, candidate) in moves.iter_mut().enumerate() {
                let flips = flips_of(candidate.square, player, opponent);
                let (next_player, next_opponent) = (opponent & !flips, player | flips | bit(candidate.square));
                let mut score;
                if i == 0 || exact_scores {
                    score = -self.negamax(next_player, next_opponent, depth - 1, -INFINITY, if exact_scores { INFINITY } else { -alpha }, false);
                } else {
                    score = -self.negamax(next_player, next_opponent, depth - 1, -alpha - 1, -alpha, false);
                    if !self.aborted && score > alpha {
                        score = -self.negamax(next_player, next_opponent, depth - 1, -INFINITY, -alpha, false);
                    }
                }
                if self.aborted {
                    complete = false;
                    break;
                }
                candidate.score = score;
                if score > iteration_score {
                    iteration_score = score;
                    iteration_best = candidate.square;
                }
                alpha = alpha.max(score);
            }

            // An unfinished iteration still counts when its first move, the best before, was searched: a move
            // that then came out best beat it.
            if iteration_best != PASS {
                result.best = iteration_best;
                result.score = iteration_score;
            }
            if !complete {
                return false;
            }
            result.depth = depth;
            moves.sort_by_key(|candidate| -candidate.score);
            if start.elapsed() > budget / 2 || is_win_score(iteration_score) || depth >= empties {
                break;
            }
        }
        true
    }

    /// Plays the game out from the root; false when it was cut short.
    fn solve_root(&mut self, root: &Position, moves: &mut Vec<RootMove>, exact_scores: bool, result: &mut SearchResult) -> bool {
        let (player, opponent) = (root.player(), root.opponent());
        let empties = root.empty_count();
        let mut alpha = -SQUARE_COUNT - 1;
        let beta = SQUARE_COUNT + 1;
        let mut solved = moves.clone();
        for (i, candidate) in solved.iter_mut().enumerate() {
            let flips = flips_of(candidate.square, player, opponent);
            let (next_player, next_opponent) = (opponent & !flips, player | flips | bit(candidate.square));
            let mut score;
            if i == 0 || exact_scores {
                score = -self.solve(next_player, next_opponent, -beta, if exact_scores { beta } else { -alpha }, empties - 1, false);
            } else {
                score = -self.solve(next_player, next_opponent, -alpha - 1, -alpha, empties - 1, false);
                if !self.aborted && score > alpha {
                    score = -self.solve(next_player, next_opponent, -beta, -alpha, empties - 1, false);
                }
            }
            if self.aborted {
                return false;
            }
            candidate.score = score;
            alpha = alpha.max(score);
        }
        solved.sort_by_key(|candidate| -candidate.score);
        *moves = solved;
        result.best = moves[0].square;
        result.score = moves[0].score;
        result.solved = true;
        result.depth = empties;
        true
    }

    pub fn search(&mut self, root: &Position, limits: &SearchLimits, stop: &Arc<AtomicBool>) -> SearchResult {
        let start = Instant::now();
        let time = Duration::from_millis(limits.milliseconds);
        self.stop = stop.clone();
        self.aborted = false;
        self.nodes = 0;

        let (player, opponent) = (root.player(), root.opponent());
        // To begin with, the moves that leave the opponent the fewest replies first.
        let mut moves: Vec<RootMove> = squares(root.moves())
            .map(|square| {
                let flips = flips_of(square, player, opponent);
                let replies = count(moves_of(opponent & !flips, player | flips | bit(square)));
                RootMove { square, score: 8 * SQUARE_VALUE[square as usize] - 64 * replies }
            })
            .collect();
        if moves.is_empty() {
            return SearchResult::default();
        }
        moves.sort_by_key(|candidate| -candidate.score);

        let mut result = SearchResult { best: moves[0].square, ..SearchResult::default() };
        if moves.len() == 1 && limits.random_margin == 0 {
            return result;
        }

        // Before an exact solution of the end, a short search puts the likely best moves first for it.
        let solve = root.empty_count() <= limits.exact_empties;
        let budget = if solve { time / 5 } else { time };
        let middle = SearchLimits { max_depth: if solve { limits.max_depth.min(10) } else { limits.max_depth }, ..*limits };
        self.deadline = start + budget;
        self.search_middle(root, &mut moves, &middle, &mut result, start, budget);
        if solve && !stop.load(Ordering::Relaxed) {
            self.deadline = start + time;
            self.aborted = false;
            self.solve_root(root, &mut moves, limits.random_margin > 0, &mut result);
        }

        if limits.random_margin > 0 && result.depth > 0 {
            // The weaker levels pick among the good moves, the better ones more often. When the end is solved, the
            // margin is counted in discs.
            let margin = if result.solved { limits.random_margin / 25 } else { limits.random_margin };
            let top = moves[0].score;
            let candidates: Vec<RootMove> = moves.iter().copied().filter(|candidate| candidate.score >= top - margin).collect();
            let weight = |candidate: &RootMove| 1.0 + f64::from(margin - (top - candidate.score)) / f64::from(margin.max(1)) * 3.0;
            let mut random = Random(limits.seed ^ root.key());
            let mut pick = random.unit() * candidates.iter().map(weight).sum::<f64>();
            let chosen = candidates
                .iter()
                .find(|candidate| {
                    pick -= weight(candidate);
                    pick < 0.0
                })
                .unwrap_or(&candidates[candidates.len() - 1]);
            result.best = chosen.square;
            result.score = chosen.score;
        }
        result.nodes = self.nodes;
        result
    }
}

/// Runs an engine on a thread of its own; the window polls for the result.
pub struct AiPlayer {
    engine: Arc<Mutex<Engine>>,
    worker: Option<JoinHandle<()>>,
    stop: Arc<AtomicBool>,
    result: Arc<Mutex<Option<SearchResult>>>,
    busy: Arc<AtomicBool>,
}

impl AiPlayer {
    pub fn new(hash_megabytes: usize) -> AiPlayer {
        AiPlayer { engine: Arc::new(Mutex::new(Engine::new(hash_megabytes))), worker: None, stop: Arc::default(), result: Arc::default(), busy: Arc::default() }
    }

    pub fn start(&mut self, position: Position, limits: SearchLimits) {
        self.cancel();
        self.busy.store(true, Ordering::Relaxed);
        self.stop = Arc::default();
        let (engine, stop, result, busy) = (self.engine.clone(), self.stop.clone(), self.result.clone(), self.busy.clone());
        self.worker = Some(std::thread::spawn(move || {
            let found = engine.lock().unwrap_or_else(PoisonError::into_inner).search(&position, &limits, &stop);
            if !stop.load(Ordering::Relaxed) {
                *result.lock().unwrap_or_else(PoisonError::into_inner) = Some(found);
            }
            busy.store(false, Ordering::Relaxed);
        }));
    }

    pub fn cancel(&mut self) {
        if let Some(worker) = self.worker.take() {
            self.stop.store(true, Ordering::Relaxed);
            let _ = worker.join();
        }
        self.busy.store(false, Ordering::Relaxed);
        self.take_result();
    }

    pub fn new_game(&mut self) {
        self.cancel();
        self.engine.lock().unwrap_or_else(PoisonError::into_inner).clear();
    }

    pub fn busy(&self) -> bool {
        self.busy.load(Ordering::Relaxed)
    }

    pub fn take_result(&mut self) -> Option<SearchResult> {
        self.result.lock().unwrap_or_else(PoisonError::into_inner).take()
    }
}

impl Drop for AiPlayer {
    fn drop(&mut self) {
        self.cancel();
    }
}
