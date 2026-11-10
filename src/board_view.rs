//! The discs as they lie and move: flying from the piles beside the board to their squares, turning over in a
//! wave that runs out from the disc placed, flying back when a move is taken back, and dissolving in when a
//! game is set out.

use crate::board_geometry::{FRAME_OUTER, TABLE_HEIGHT, square_center};
use crate::math::{Mat4, PI, Vec3, smooth_step, vec3};
use crate::piece_models::{HALF_HEIGHT, RADIUS};
use crate::renderer::{ContactShadow, DiscDraw};
use crate::reversi::{self, Color, Game, MoveRecord, SQUARE_COUNT, Square, file_of, row_of, squares};

/// What happened in a frame, for the sounds.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Event {
    Place,
    Flip,
    Stack,
}

const FADE_TIME: f32 = 0.45;
const FLIGHT_TIME: f32 = 0.55;
const TURN_TIME: f32 = 0.36;
const TURN_LIFT: f32 = 0.42; // enough for a disc on its edge to clear the board
const WAVE_STEP: f32 = 0.075; // between the discs one square further from the one placed
const STACK_HEIGHT: usize = 10; // discs in a stack beside the board
const THICKNESS: f32 = 2.0 * HALF_HEIGHT;

fn ease(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

fn approach(value: f32, target: f32, step: f32) -> f32 {
    if value < target { (value + step).min(target) } else { (value - step).max(target) }
}

fn pile_of(color: Color) -> usize {
    match color {
        Color::Black => 0,
        Color::White => 1,
    }
}

/// The level axis a disc turns about to roll away from a point.
fn axis_away_from(from: Vec3, to: Vec3) -> Vec3 {
    let direction = vec3(to.x - from.x, 0.0, to.z - from.z);
    if direction.length() < 1e-4 {
        return Vec3::X;
    }
    let direction = direction.normalized();
    vec3(direction.z, 0.0, -direction.x)
}

fn ground_at(position: Vec3) -> f32 {
    if position.x.abs() < FRAME_OUTER && position.z.abs() < FRAME_OUTER { 0.0 } else { TABLE_HEIGHT }
}

/// Each player's discs stand in stacks beside the board, on the player's right: Black's on the right of the
/// default view, the stack nearest the player used first.
fn pile_position(pile: usize, level: usize) -> Vec3 {
    let stack = ((level / STACK_HEIGHT) as f32 - 1.0) * 1.05;
    let y = TABLE_HEIGHT + (level % STACK_HEIGHT) as f32 * THICKNESS;
    if pile == 0 { vec3(5.35, y, stack) } else { vec3(-5.35, y, -stack) }
}

/// Where a disc is in the game.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Place {
    Square(Square),
    Pile,
    Leaving, // dissolving, when a new game is set out
}

#[derive(Clone)]
struct Disc {
    place: Place,
    pile: Option<usize>, // in a pile, Black's (0) or White's (1); on the board, the pile it came from
    level: usize,        // its place in the pile, counted from the bottom
    white_up: bool,      // the face up when it lies still
    yaw: f32,            // each disc turned its own way, so that the grain differs
    seed: f32,
    position: Vec3, // the middle of its underside
    // A flight from one place to another, in an arc.
    from: Vec3,
    to: Vec3,
    flight_time: f32,
    flight_duration: f32,
    flight_delay: f32,
    arc: f32,
    flying: bool,
    landing: Event,
    // Turning over about a level axis, lifted off the board for the turn.
    turn_axis: Vec3,
    turn_time: f32,
    turn_duration: f32,
    turn_delay: f32,
    turn_lift: f32,
    turning: bool,
    dissolve: f32,
    dissolve_target: f32,
    dissolve_delay: f32,
    remove_when_gone: bool,
    // Leaning in the preview of a move that would turn it.
    lean_axis: Vec3,
    lean: f32,
    lean_target: f32,
}

impl Disc {
    fn fly(&mut self, to: Vec3, duration: f32, arc: f32, delay: f32, landing: Event) {
        self.from = self.position;
        self.to = to;
        self.flight_time = 0.0;
        self.flight_duration = duration;
        self.flight_delay = delay;
        self.arc = arc;
        self.flying = true;
        self.landing = landing;
    }

    /// Turns the disc to show a face, unless it shows it already or is about to.
    fn turn_to(&mut self, white_up: bool, axis: Vec3, duration: f32, lift: f32, delay: f32) {
        let ending = if self.turning { !self.white_up } else { self.white_up };
        if ending == white_up {
            return;
        }
        if self.turning {
            self.turning = false;
            if self.turn_delay > 0.0 {
                return; // a turn not yet begun is simply called off
            }
            self.white_up = !self.white_up; // one under way is finished at once, and the disc turns back
        }
        self.turning = true;
        self.turn_axis = axis;
        self.turn_time = 0.0;
        self.turn_duration = duration;
        self.turn_delay = delay;
        self.turn_lift = lift;
    }

    /// How far the disc is off the board for its turn.
    fn lift(&self) -> f32 {
        if !self.turning || self.turn_delay > 0.0 {
            return 0.0;
        }
        self.turn_lift * (PI * (self.turn_time / self.turn_duration).min(1.0)).sin()
    }
}

pub struct BoardView {
    discs: Vec<Disc>,
    pile_counts: [usize; 2],
    seed: u32,
}

impl BoardView {
    pub fn new() -> BoardView {
        BoardView { discs: Vec::new(), pile_counts: [0; 2], seed: 1 }
    }

    fn random(&mut self) -> f32 {
        self.seed = self.seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.seed >> 8) as f32 / (1u32 << 24) as f32
    }

    /// A disc lying at a place; one that appears dissolves in after the delay.
    fn add(&mut self, place: Place, pile: Option<usize>, level: usize, white_up: bool, position: Vec3, appear: Option<f32>) -> usize {
        let yaw = self.random() * 2.0 * PI;
        let seed = self.random();
        self.discs.push(Disc {
            place,
            pile,
            level,
            white_up,
            yaw,
            seed,
            position,
            from: position,
            to: position,
            flight_time: 0.0,
            flight_duration: 0.0,
            flight_delay: 0.0,
            arc: 0.0,
            flying: false,
            landing: Event::Place,
            turn_axis: Vec3::X,
            turn_time: 0.0,
            turn_duration: 0.0,
            turn_delay: 0.0,
            turn_lift: 0.0,
            turning: false,
            dissolve: if appear.is_some() { 1.0 } else { 0.0 },
            dissolve_target: 0.0,
            dissolve_delay: appear.unwrap_or(0.0),
            remove_when_gone: false,
            lean_axis: Vec3::X,
            lean: 0.0,
            lean_target: 0.0,
        });
        self.discs.len() - 1
    }

    fn on_square(&self, square: Square) -> Option<usize> {
        self.discs.iter().position(|disc| disc.place == Place::Square(square))
    }

    fn top_of_pile(&self, pile: usize) -> Option<usize> {
        let in_pile = |disc: &Disc| disc.place == Place::Pile && disc.pile == Some(pile);
        (0..self.discs.len()).filter(|index| in_pile(&self.discs[*index])).max_by_key(|index| self.discs[*index].level)
    }

    /// The pile a disc of a side is taken from: its own, or the other's when that is used up.
    fn pile_for(&self, side: Color) -> usize {
        let pile = pile_of(side);
        if self.pile_counts[pile] == 0 { 1 - pile } else { pile }
    }

    /// Sets out the game's position, and beside the board the discs not yet played.
    pub fn rebuild(&mut self, game: &Game, animate: bool) {
        if animate {
            for disc in &mut self.discs {
                disc.dissolve_target = 1.0;
                disc.dissolve_delay = 0.0;
                disc.remove_when_gone = true;
                disc.place = Place::Leaving;
                disc.pile = None;
            }
        } else {
            self.discs.clear();
        }

        // The discs off the board at the start, shared out between the players, and then the pile each disc
        // played came from: a player whose own pile is used up takes from the other's.
        let start = game.start();
        let total = start.empty_count() as usize;
        self.pile_counts[0] = (total + usize::from(start.side_to_move() == Color::Black)) / 2;
        self.pile_counts[1] = total - self.pile_counts[0];
        let mut origin = [None; SQUARE_COUNT as usize];
        for record in game.moves().iter().filter(|record| !record.is_pass()) {
            let pile = self.pile_for(record.side);
            self.pile_counts[pile] -= 1;
            origin[record.square as usize] = Some(pile);
        }

        let position = game.current();
        for square in 0..SQUARE_COUNT {
            if let Some(color) = position.at(square) {
                let appear = animate.then_some(0.3 + 0.01 * square as f32);
                self.add(Place::Square(square), origin[square as usize], 0, color == Color::White, square_center(square), appear);
            }
        }
        for pile in 0..2 {
            for level in 0..self.pile_counts[pile] {
                let appear = animate.then_some(0.5 + 0.012 * level as f32);
                self.add(Place::Pile, Some(pile), level, pile == 1, pile_position(pile, level), appear);
            }
        }
    }

    /// A move played (forward) or taken back.
    pub fn play_move(&mut self, record: &MoveRecord, forward: bool) {
        if record.is_pass() {
            return;
        }
        let center = square_center(record.square);
        let mover_white = record.side == Color::White;
        let distance = |square: Square| (file_of(square) - file_of(record.square)).abs().max((row_of(square) - row_of(record.square)).abs());

        if forward {
            // A disc from the player's pile, or the other's when that is used up, flies to the square.
            let pile = self.pile_for(record.side);
            let index = match self.top_of_pile(pile) {
                Some(index) => index,
                None => self.add(Place::Pile, Some(pile), 0, mover_white, pile_position(pile, 0), None),
            };
            self.pile_counts[pile] = self.pile_counts[pile].saturating_sub(1);
            let disc = &mut self.discs[index];
            disc.place = Place::Square(record.square);
            disc.pile = Some(pile);
            let axis = axis_away_from(disc.position, center);
            disc.fly(center, FLIGHT_TIME, 1.1, 0.0, Event::Place);
            disc.turn_to(mover_white, axis, FLIGHT_TIME * 0.8, 0.0, 0.05);

            // Then the discs it brackets turn over, the nearest first.
            for square in squares(record.flipped) {
                if let Some(turned) = self.on_square(square) {
                    let delay = FLIGHT_TIME + WAVE_STEP * (distance(square) - 1) as f32;
                    self.discs[turned].turn_to(mover_white, axis_away_from(center, square_center(square)), TURN_TIME, TURN_LIFT, delay);
                }
            }
            return;
        }

        // Back: the turned discs turn back, the furthest first, and then the disc placed returns to its pile.
        let furthest = squares(record.flipped).map(distance).max().unwrap_or(0);
        for square in squares(record.flipped) {
            if let Some(turned) = self.on_square(square) {
                let delay = WAVE_STEP * (furthest - distance(square)) as f32;
                self.discs[turned].turn_to(!mover_white, axis_away_from(square_center(square), center), TURN_TIME, TURN_LIFT, delay);
            }
        }
        if let Some(index) = self.on_square(record.square) {
            let pile = self.discs[index].pile.unwrap_or(pile_of(record.side));
            let level = self.pile_counts[pile];
            self.pile_counts[pile] += 1;
            let place = pile_position(pile, level);
            let delay = WAVE_STEP * furthest as f32 + if record.flipped != 0 { TURN_TIME * 0.6 } else { 0.0 };
            let disc = &mut self.discs[index];
            disc.place = Place::Pile;
            disc.pile = Some(pile);
            disc.level = level;
            disc.fly(place, FLIGHT_TIME, 1.1, delay, Event::Stack);
            disc.turn_to(pile == 1, axis_away_from(center, place), FLIGHT_TIME * 0.8, 0.0, delay + 0.05);
        }
    }

    /// The discs that a disc placed on `square` would turn over lean away from it, about to go, and glow.
    pub fn preview(&mut self, square: Option<Square>, turning: u64) {
        for disc in &mut self.discs {
            disc.lean_target = 0.0;
            if let (Some(square), Place::Square(own)) = (square, disc.place) {
                if turning & reversi::bit(own) != 0 {
                    disc.lean_target = 1.0;
                    disc.lean_axis = axis_away_from(square_center(square), square_center(own));
                }
            }
        }
    }

    pub fn update(&mut self, seconds: f32, events: &mut Vec<Event>) {
        for disc in &mut self.discs {
            disc.lean = approach(disc.lean, disc.lean_target, seconds / 0.15);
            if disc.dissolve_delay > 0.0 {
                disc.dissolve_delay -= seconds;
            } else {
                disc.dissolve = approach(disc.dissolve, disc.dissolve_target, seconds / FADE_TIME);
            }

            if disc.flying {
                if disc.flight_delay > 0.0 {
                    disc.flight_delay -= seconds;
                } else {
                    disc.flight_time = (disc.flight_time + seconds).min(disc.flight_duration);
                    let t = disc.flight_time / disc.flight_duration;
                    disc.position = disc.from.lerp(disc.to, ease(t)) + vec3(0.0, disc.arc * (PI * t).sin(), 0.0);
                    if t >= 1.0 {
                        disc.position = disc.to;
                        disc.flying = false;
                        events.push(disc.landing);
                    }
                }
            }

            if disc.turning {
                if disc.turn_delay > 0.0 {
                    disc.turn_delay -= seconds;
                } else {
                    disc.turn_time += seconds;
                    if disc.turn_time >= disc.turn_duration {
                        disc.white_up = !disc.white_up;
                        disc.turning = false;
                        if disc.turn_lift > 0.0 {
                            events.push(Event::Flip);
                        }
                    }
                }
            }
        }
        self.discs.retain(|disc| !(disc.remove_when_gone && disc.dissolve >= 1.0));
    }

    /// Nothing flying, turning or dissolving: the position shown is the game's.
    pub fn settled(&self) -> bool {
        !self.discs.iter().any(|disc| disc.flying || disc.turning || disc.dissolve != disc.dissolve_target)
    }

    pub fn animating(&self) -> bool {
        !self.settled() || self.discs.iter().any(|disc| disc.lean != disc.lean_target)
    }

    pub fn draws(&self, last_move: Option<Square>, accent: Vec3, time: f32) -> Vec<DiscDraw> {
        self.discs
            .iter()
            .map(|disc| {
                let angle = if disc.turning && disc.turn_delay <= 0.0 { PI * ease((disc.turn_time / disc.turn_duration).min(1.0)) } else { 0.0 };
                let lean = ease(disc.lean) * (0.85 + 0.15 * (time * 5.0).sin());
                let face = if disc.white_up { Mat4::IDENTITY } else { Mat4::rotation_x(PI) };
                let on_last_move = matches!(disc.place, Place::Square(square) if Some(square) == last_move);
                DiscDraw {
                    model: Mat4::translation(disc.position + vec3(0.0, HALF_HEIGHT + disc.lift() + 0.09 * lean, 0.0))
                        * Mat4::rotation(disc.lean_axis, 0.28 * lean)
                        * Mat4::rotation(disc.turn_axis, angle)
                        * face
                        * Mat4::rotation_y(disc.yaw),
                    ground: ground_at(disc.position),
                    dissolve: disc.dissolve,
                    seed: disc.seed,
                    glow_color: accent,
                    glow: 0.7 * lean,
                    mark: if on_last_move && !disc.flying { 1.0 } else { 0.0 },
                    opacity: 1.0,
                }
            })
            .collect()
    }

    pub fn contacts(&self) -> Vec<ContactShadow> {
        self.discs
            .iter()
            // In a stack only the lowest disc touches the table.
            .filter(|disc| !(disc.place == Place::Pile && !disc.flying && disc.level % STACK_HEIGHT != 0))
            .filter_map(|disc| {
                let height = disc.position.y - ground_at(disc.position) + disc.lift() + 0.09 * disc.lean;
                let strength = 0.72 * (1.0 - smooth_step(0.0, 0.8, height)) * (1.0 - disc.dissolve);
                (strength > 0.01).then_some(ContactShadow { x: disc.position.x, z: disc.position.z, radius: RADIUS * (1.05 + height * 0.6), strength })
            })
            .collect()
    }
}
