//! The game's window: it runs the game, the computer players and the interface, and draws them.

mod widgets;
mod window;

use std::hash::{BuildHasher, Hasher};
use std::time::Instant;

use winit::event_loop::ActiveEventLoop;
use winit::keyboard::ModifiersState;

use crate::Result;
use crate::board_geometry as board;
use crate::board_view::{BoardView, Event};
use crate::camera::OrbitCamera;
use crate::engine::{AiPlayer, SearchLimits, SearchResult};
use crate::gfx;
use crate::math::{Mat4, PI, Vec3, vec3};
use crate::piece_models::{self as pieces, HALF_HEIGHT};
use crate::platform::GlWindow;
use crate::renderer::{BoardMarks, DiscDraw, FrostedPanel, Renderer, Scene};
use crate::reversi::{self, Color, Game, GameResult, GameStatus, MoveRecord, SQUARE_COUNT, Square, square_name};
use crate::settings::{Settings, WindowPlacement};
use crate::sound::{Effect, SoundBank};
use crate::strings::{Language, Str, tr};
use crate::text::FontLibrary;
use crate::themes::{THEME_COUNT, themes};
use crate::ui::{self, FontId, Painter, Rect, rect, rgb, rgba};

pub use window::run;

use widgets::{MUTED_COLOR, TEXT_COLOR, Toast};
use window::{Key, Pointer};

const TITLE: &str = "3D Reversi";
// The window's size where none is remembered, and its smallest, on a screen of the usual scale.
const DEFAULT_SIZE: (f64, f64) = (1500.0, 920.0);
const MIN_SIZE: (f64, f64) = (900.0, 600.0);

// The widgets; a selector takes two numbers, one for each of its arrows.
const HELP_BUTTON: i32 = 1;
const UNDO_BUTTON: i32 = 2;
const REDO_BUTTON: i32 = 3;
const HINT_BUTTON: i32 = 4;
const NEW_GAME_BUTTON: i32 = 5;
const FLIP_BUTTON: i32 = 6;
const MODE_SELECTOR: i32 = 7;
const LEVEL_SELECTOR: i32 = 9;
const MOVES_SELECTOR: i32 = 11;
const THEME_SELECTOR: i32 = 13;
const SOUND_BUTTON: i32 = 15;
const LANGUAGE_BUTTON: i32 = 16;
const HELP_CLOSE: i32 = 101;

const BLACK_DISC: ui::Color = rgb(22, 22, 26);
const WHITE_DISC: ui::Color = rgb(238, 236, 230);
const DISC_RIM: ui::Color = rgb(120, 125, 140);

const HELP_WIDTH: f32 = 680.0;
const HELP_ROW: f32 = 27.0;
const HELP_PAD: f32 = 28.0;

/// The keys of the help, each with what it does.
const HELP_LINES: [(Option<Str>, &str, Str); 20] = [
    (Some(Str::KeyClick), "", Str::HelpClick),
    (Some(Str::KeyDrag), "", Str::HelpRotate),
    (Some(Str::KeyWheel), "", Str::HelpZoom),
    (None, "Ctrl+Z  ←  Backspace", Str::HelpUndo),
    (None, "Ctrl+Y  →", Str::HelpRedo),
    (None, "H", Str::HelpHint),
    (None, "N", Str::HelpNew),
    (None, "F", Str::HelpFlip),
    (None, "V", Str::HelpTopView),
    (None, "1 – 5", Str::HelpLevel),
    (None, "M", Str::HelpMode),
    (None, "D", Str::HelpShowMoves),
    (None, "T", Str::HelpTheme),
    (None, "S", Str::HelpSound),
    (None, "L", Str::HelpLanguage),
    (None, "Tab", Str::HelpPanel),
    (None, "F11  Alt+Enter", Str::HelpFullScreen),
    (None, "Ctrl+C", Str::HelpCopy),
    (None, "Ctrl+V", Str::HelpPaste),
    (None, "Esc  F1", Str::HelpClose),
];

/// Who plays whom.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    PlayBlack,
    PlayWhite,
    TwoPlayers,
    Demo,
}

impl Mode {
    const ALL: [Mode; 4] = [Mode::PlayBlack, Mode::PlayWhite, Mode::TwoPlayers, Mode::Demo];

    /// The mode a number of steps on, round and round.
    fn stepped(self, steps: i32) -> Mode {
        Mode::ALL[(self as i32 + steps).rem_euclid(4) as usize]
    }

    fn against_computer(self) -> bool {
        matches!(self, Mode::PlayBlack | Mode::PlayWhite)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Gesture {
    None,
    PressSquare,
    TurnCamera,
}

/// Depth, time, the depth of the exact ending, and the randomness of each level.
fn level_limits(level: i32, ply: usize, seed: u64) -> SearchLimits {
    let (max_depth, milliseconds, exact_empties, random_margin) = match level {
        1 => (1, 2000, 0, 900),
        2 => (2, 2000, 8, 300),
        3 => (4, 2000, 12, 80),
        4 => (8, 1500, 16, 0),
        _ => (60, 3500, 20, 0),
    };
    // The first moves vary a little at every level, so that the games differ.
    let random_margin = if ply < 6 { random_margin.max(60) } else { random_margin };
    SearchLimits { max_depth, milliseconds, exact_empties, random_margin, seed }
}

/// A number that differs from run to run and from call to call.
fn random_seed() -> u64 {
    std::collections::hash_map::RandomState::new().build_hasher().finish()
}

fn disc_color(side: Color) -> ui::Color {
    match side {
        Color::Black => BLACK_DISC,
        Color::White => WHITE_DISC,
    }
}

/// A little disc for the panel, with a rim and a gleam.
fn draw_disc(painter: &mut Painter, x: f32, y: f32, radius: f32, side: Color) {
    painter.disc(x, y, radius, DISC_RIM);
    painter.disc(x, y, radius - (radius * 0.1).max(1.0), disc_color(side));
    painter.disc(x - radius * 0.28, y - radius * 0.3, radius * 0.38, rgba(255, 255, 255, if side == Color::Black { 46 } else { 120 }));
}

fn result_title(result: GameResult) -> Str {
    match result {
        GameResult::BlackWins => Str::BlackWins,
        GameResult::WhiteWins => Str::WhiteWins,
        _ => Str::Draw,
    }
}

pub struct App {
    renderer: Renderer,
    painter: Painter,
    fonts: FontLibrary,
    view: BoardView,
    sound: SoundBank,
    clipboard: Option<arboard::Clipboard>,
    settings: Settings,
    language: Language,
    width: i32,
    height: i32,
    scale: f32,      // of the interface: 1 at 96 dots per inch
    font_scale: f32, // the scale the fonts were made for
    dirty: bool,     // something to draw since the last frame
    idle: bool,      // waiting for input: the time waited is not animation time
    full_screen: bool,
    maximized: bool,
    placement: Option<WindowPlacement>,
    last_frame: Instant,
    time: f32,

    game: Game,
    camera: OrbitCamera,
    mode: Mode,
    level: i32,
    theme: usize,
    show_moves: bool,
    computer: AiPlayer,
    hinter: AiPlayer,
    computer_thinking: bool,
    computer_clock: f32,
    computer_result: Option<SearchResult>,
    hint_thinking: bool,
    hint: Option<Square>,
    hint_glow: f32,
    announced_end: bool,
    events: Vec<Event>,

    pointer: Pointer,
    modifiers: ModifiersState,
    gesture: Gesture,
    press_x: f32,
    press_y: f32,
    // The pointer when the camera last turned.
    drag_x: f32,
    drag_y: f32,
    press_square: Option<Square>,
    hover_square: Option<Square>, // a legal square under the pointer
    cursor_hand: bool,
    shown_cursor_hand: bool,

    active_widget: i32,
    panel_visible: bool,
    panel_slide: f32,
    help_visible: bool,
    move_scroll: f32,
    follow_moves: bool,
    banner_fade: f32,
    banner_status: GameStatus,
    toasts: Vec<Toast>,

    // The window goes last: what was made in its OpenGL context is dropped before it.
    gl: GlWindow,
}

impl App {
    fn new(event_loop: &ActiveEventLoop) -> Result<App> {
        let settings = Settings::load();
        let gl = window::open(event_loop, settings.window, settings.maximized)?;
        gfx::set_glsl_version(gl.version());

        let mut painter = Painter::new()?;
        let mut fonts = FontLibrary::new();
        let scale = gl.window.scale_factor() as f32;
        painter.set_fonts(widgets::build_fonts(&mut fonts, scale)?);
        gl.window.set_visible(true);
        let size = gl.window.inner_size();
        let (width, height) = (size.width as i32, size.height as i32);
        let language = settings.language;
        widgets::show_loading(&gl, &mut painter, tr(Str::Title, language));

        let theme = settings.theme as usize;
        let renderer = Renderer::new(&pieces::build_disc(), &mut fonts, &themes()[theme])?;
        let mode = Mode::ALL[settings.mode as usize];
        let game = Game::deserialize(&settings.game).unwrap_or_else(Game::new);
        let mut view = BoardView::new();
        view.rebuild(&game, true);
        let mut sound = SoundBank::new();
        sound.enabled = settings.sound;

        let mut app = App {
            renderer,
            painter,
            fonts,
            view,
            sound,
            clipboard: None,
            language,
            width,
            height,
            scale,
            font_scale: scale,
            dirty: true,
            idle: false,
            full_screen: false,
            maximized: settings.maximized,
            placement: settings.window,
            last_frame: Instant::now(),
            time: 0.0,
            announced_end: game.status().over(),
            game,
            camera: OrbitCamera::new(),
            mode,
            level: settings.level,
            theme,
            show_moves: settings.show_moves,
            computer: AiPlayer::new(32),
            hinter: AiPlayer::new(16),
            computer_thinking: false,
            computer_clock: 0.0,
            computer_result: None,
            hint_thinking: false,
            hint: None,
            hint_glow: 0.0,
            events: Vec::new(),
            pointer: Pointer::default(),
            modifiers: ModifiersState::empty(),
            gesture: Gesture::None,
            press_x: 0.0,
            press_y: 0.0,
            drag_x: 0.0,
            drag_y: 0.0,
            press_square: None,
            hover_square: None,
            cursor_hand: false,
            shown_cursor_hand: false,
            active_widget: 0,
            panel_visible: settings.panel,
            panel_slide: if settings.panel { 1.0 } else { 0.0 },
            help_visible: false,
            move_scroll: 0.0,
            follow_moves: true,
            banner_fade: 0.0,
            banner_status: GameStatus::default(),
            toasts: Vec::new(),
            settings,
            gl,
        };
        app.camera.reset(app.view_side());
        app.track_placement();
        Ok(app)
    }

    fn frame(&mut self) -> Result<()> {
        let Some(seconds) = self.begin_frame()? else { return Ok(()) };
        self.renderer.resize(self.width, self.height)?;

        self.update(seconds);

        let theme = self.theme();
        let position = *self.game.current();
        let ready = self.human_to_move() && self.view.settled();
        let last_move = self.game.moves().iter().rev().find(|record| !record.is_pass()).map(|record| record.square);

        let mut marks = BoardMarks::default();
        let mut ghost = None;
        if ready && self.show_moves {
            marks.legal = position.moves();
            if let Some(square) = self.hover_square {
                // The disc that a click would place, seen through.
                marks.hover = reversi::bit(square);
                let face = if position.side_to_move() == Color::White { Mat4::IDENTITY } else { Mat4::rotation_x(PI) };
                ghost = Some(DiscDraw {
                    model: Mat4::translation(board::square_center(square) + vec3(0.0, HALF_HEIGHT + 0.04, 0.0)) * face,
                    ground: 0.0,
                    glow_color: Vec3::ZERO,
                    glow: 0.0,
                    dissolve: 0.0,
                    seed: 0.0,
                    mark: 0.0,
                    opacity: 0.55,
                });
            }
        }
        if let Some(square) = last_move {
            marks.last_move = reversi::bit(square);
        }
        if let (Some(square), true) = (self.hint, self.hint_glow > 0.01) {
            marks.hint = reversi::bit(square);
        }
        let scene = Scene {
            view: self.camera.view(),
            projection: self.camera.projection(self.width as f32 / self.height as f32),
            eye: self.camera.eye(),
            time: self.time,
            discs: self.view.draws(last_move, theme.accent, self.time),
            ghost,
            contacts: self.view.contacts(),
            marks,
            panels: self.frosted_panels(),
        };
        self.renderer.render(&scene);

        self.cursor_hand = false;
        self.painter.begin(self.width, self.height);
        self.draw_interface();
        self.painter.flush();
        self.handle_board_pointer();

        self.end_frame();
        Ok(())
    }

    fn update(&mut self, seconds: f32) {
        let space = self.slide_panel(seconds);
        self.camera.set_space(space);
        self.camera.update(seconds);

        // The discs that the move under the pointer would turn over lean, about to go.
        let ready = self.human_to_move() && self.view.settled();
        let previewed = self.hover_square.filter(|_| ready && self.show_moves);
        self.view.preview(previewed, previewed.map_or(0, |square| self.game.current().flips(square)));

        // One sound of each kind a frame: the discs of a wave land together.
        self.events.clear();
        self.view.update(seconds, &mut self.events);
        self.events.sort_unstable();
        self.events.dedup();
        for event in &self.events {
            self.sound.play(match event {
                Event::Place => Effect::Place,
                Event::Flip => Effect::Flip,
                Event::Stack => Effect::Stack,
            });
        }

        let over = self.banner_wanted();
        if over {
            self.banner_status = self.game.status(); // kept while the banner fades after a new game
        }
        self.banner_fade = (self.banner_fade + if over { seconds } else { -seconds } * 3.0).clamp(0.0, 1.0);
        self.hint_glow = (self.hint_glow + if self.hint.is_some() { seconds } else { -seconds } * 4.0).clamp(0.0, 1.0);
        self.age_toasts(seconds);

        if self.hint_thinking {
            if let Some(result) = self.hinter.take_result() {
                self.hint_thinking = false;
                if result.best != reversi::PASS && self.human_to_move() {
                    self.hint = Some(result.best);
                    self.show_toast(format!("{}{}", self.t(Str::HintToast), square_name(result.best)));
                }
            } else if !self.hinter.busy() {
                self.hint_thinking = false;
            }
        }

        self.update_computer(seconds);

        if over && !self.announced_end {
            self.announced_end = true;
            self.sound.play(Effect::GameOver);
        }
        if !self.game.status().over() {
            self.announced_end = false;
        }
    }

    /// Whether the banner of the game's end is to be seen: the game is over, and the last discs have come to rest.
    fn banner_wanted(&self) -> bool {
        self.game.status().over() && self.view.settled()
    }

    fn needs_animation(&self) -> bool {
        // What slides and fades is held against where it is going: it may be at rest when its goal changes.
        let panel_target = if self.panel_visible { 1.0 } else { 0.0 };
        let banner_target = if self.banner_wanted() { 1.0 } else { 0.0 };
        self.camera.moving()
            || self.view.animating()
            || self.computer_thinking
            || self.computer_result.is_some()
            || self.hint_thinking
            || !self.toasts.is_empty()
            || self.panel_slide != panel_target
            || self.banner_fade != banner_target
            || self.hint.is_some()
            || self.hint_glow > 0.0
            || self.gesture != Gesture::None
            || self.pointer.left_down
            || self.pointer.right_down
            || self.hover_square.is_some()
            || (!self.game.status().over() && !self.is_human(self.game.current().side_to_move()))
    }

    // The game ---------------------------------------------------------------------------------------

    fn is_human(&self, side: Color) -> bool {
        match self.mode {
            Mode::PlayBlack => side == Color::Black,
            Mode::PlayWhite => side == Color::White,
            Mode::TwoPlayers => true,
            Mode::Demo => false,
        }
    }

    fn human_to_move(&self) -> bool {
        !self.game.status().over() && self.is_human(self.game.current().side_to_move())
    }

    fn view_side(&self) -> Color {
        if self.mode == Mode::PlayWhite { Color::White } else { Color::Black }
    }

    fn stop_computer(&mut self) {
        self.computer.cancel();
        self.computer_thinking = false;
        self.computer_result = None;
        self.hinter.cancel();
        self.hint_thinking = false;
    }

    fn update_computer(&mut self, seconds: f32) {
        if self.game.status().over() || self.is_human(self.game.current().side_to_move()) {
            return;
        }
        self.computer_clock += seconds;
        if !self.computer_thinking && self.computer_result.is_none() {
            self.computer.start(*self.game.current(), level_limits(self.level, self.game.ply(), random_seed()));
            self.computer_thinking = true;
            self.computer_clock = 0.0;
        }
        if self.computer_thinking {
            if let Some(result) = self.computer.take_result() {
                self.computer_result = Some(result);
                self.computer_thinking = false;
            } else if !self.computer.busy() {
                self.computer_thinking = false; // cancelled; a new search starts next frame
            }
        }
        // The computer waits for the discs to settle, and a moment more, so that its move can be followed.
        let pause = if self.mode == Mode::Demo { 0.8 } else { 0.35 };
        if self.view.settled() && self.computer_clock >= pause && self.gesture == Gesture::None {
            if let Some(result) = self.computer_result.take() {
                if result.best != reversi::PASS {
                    self.play_move(result.best);
                }
            }
        }
    }

    fn new_game(&mut self) {
        self.stop_computer();
        self.computer.new_game();
        self.game = Game::new();
        self.view.rebuild(&self.game, true);
        self.camera.reset(self.view_side());
        self.announced_end = false;
        self.after_position_change();
    }

    fn play_move(&mut self, square: Square) {
        if !self.game.play(square) {
            return;
        }
        // The disc, and the pass it may have forced.
        let moves = self.game.moves();
        let played = if moves.last().is_some_and(MoveRecord::is_pass) { 2 } else { 1 };
        for record in &moves[moves.len() - played..] {
            self.view.play_move(record, true);
        }
        self.announce_pass();
        self.after_position_change();
    }

    /// A toast for the pass, if the side to move just passed.
    fn announce_pass(&mut self) {
        let Some(last) = self.game.moves().last().copied() else { return };
        if !last.is_pass() || self.game.status().over() {
            return;
        }
        self.show_toast(self.t(if last.side == Color::Black { Str::BlackPasses } else { Str::WhitePasses }));
        self.sound.play(Effect::Pass);
    }

    /// Takes back a disc, with the pass after it, in the game and on the board.
    fn undo_one(&mut self) {
        let before = self.game.moves().to_vec();
        self.game.undo();
        for record in before[self.game.ply()..].iter().rev() {
            self.view.play_move(record, false);
        }
    }

    fn redo_one(&mut self) {
        let before = self.game.ply();
        self.game.redo();
        for record in &self.game.moves()[before..] {
            self.view.play_move(record, true);
        }
    }

    fn undo(&mut self) {
        self.stop_computer();
        if !self.game.can_undo() {
            self.show_toast(self.t(Str::NothingToUndo));
            return;
        }
        self.undo_one();
        // Against the computer, back to the player's own turn.
        while self.mode.against_computer() && !self.is_human(self.game.current().side_to_move()) && self.game.can_undo() {
            self.undo_one();
        }
        self.after_position_change();
    }

    fn redo(&mut self) {
        self.stop_computer();
        if !self.game.can_redo() {
            return;
        }
        self.redo_one();
        while self.mode.against_computer() && !self.is_human(self.game.current().side_to_move()) && self.game.can_redo() {
            self.redo_one();
        }
        self.after_position_change();
    }

    fn set_mode(&mut self, mode: Mode) {
        self.stop_computer();
        self.mode = mode;
        if mode.against_computer() {
            self.camera.face_side(self.view_side());
        }
        self.hint = None;
        self.save();
    }

    fn set_level(&mut self, level: i32) {
        self.level = level.clamp(1, 5);
        if self.computer_thinking {
            self.stop_computer(); // starts again at the new level
        }
        self.save();
    }

    fn set_theme(&mut self, theme: i32) {
        self.theme = theme.rem_euclid(THEME_COUNT as i32) as usize;
        self.renderer.set_theme(self.theme());
        self.save();
    }

    fn toggle_show_moves(&mut self) {
        self.show_moves = !self.show_moves;
        self.save();
    }

    fn toggle_sound(&mut self) {
        self.sound.enabled = !self.sound.enabled;
        self.save();
    }

    fn toggle_language(&mut self) {
        self.language = self.language.other();
        self.save();
    }

    fn request_hint(&mut self) {
        if !self.human_to_move() || self.hint_thinking {
            return;
        }
        self.hint = None;
        self.hinter.start(*self.game.current(), SearchLimits { max_depth: 60, milliseconds: 1500, exact_empties: 18, random_margin: 0, seed: 0 });
        self.hint_thinking = true;
    }

    fn after_position_change(&mut self) {
        self.hint = None;
        self.hover_square = None;
        self.follow_moves = true;
        self.save();
    }

    fn copy_game(&mut self, position: bool) {
        let text = if position { self.game.current().to_text() } else { self.game.to_text() };
        if self.copy_text(text) {
            self.show_toast(self.t(if position { Str::PositionCopied } else { Str::GameCopied }));
        }
    }

    fn paste_game(&mut self) {
        let Ok(game) = Game::parse(&self.pasted_text()) else {
            self.show_toast(self.t(Str::NothingToPaste));
            return;
        };
        self.stop_computer();
        self.computer.new_game();
        self.game = game;
        self.view.rebuild(&self.game, true);
        self.announced_end = self.game.status().over();
        self.after_position_change();
        self.show_toast(self.t(Str::GameLoaded));
    }

    fn save(&mut self) {
        self.settings.theme = self.theme as i32;
        self.settings.level = self.level;
        self.settings.mode = self.mode as i32;
        self.settings.show_moves = self.show_moves;
        self.settings.sound = self.sound.enabled;
        self.settings.panel = self.panel_visible;
        self.settings.language = self.language;
        self.settings.game = self.game.serialize();
        self.settings.window = self.placement;
        self.settings.maximized = self.maximized;
        self.settings.save();
    }

    // Input ------------------------------------------------------------------------------------------

    /// Where the ray through the pointer meets the plane at a height.
    fn pointer_on_plane(&self, height: f32) -> Vec3 {
        let (origin, direction) = self.camera.ray(self.pointer.x, self.pointer.y, self.width as f32, self.height as f32);
        if direction.y.abs() < 1e-5 {
            return origin;
        }
        let t = (height - origin.y) / direction.y;
        origin + direction * t.max(0.0)
    }

    fn square_under_pointer(&self) -> Option<Square> {
        let square = board::square_at(self.pointer_on_plane(0.0));
        (square >= 0).then_some(square)
    }

    fn handle_board_pointer(&mut self) {
        let blocked = self.pointer.consumed || self.help_visible;
        let scale = self.scale;
        let ready = self.human_to_move() && self.view.settled();
        let position = *self.game.current();
        let inside_window = self.pointer.x >= 0.0 && self.pointer.y >= 0.0 && self.pointer.x < self.width as f32 && self.pointer.y < self.height as f32;
        let square = if inside_window && !blocked { self.square_under_pointer() } else { None };
        let legal = |square: Option<Square>| ready && square.is_some_and(|square| position.is_legal(square));

        // The legal square under the pointer, previewed while the legal moves are shown.
        self.hover_square = square.filter(|_| legal(square) && self.gesture != Gesture::TurnCamera);
        if self.hover_square.is_some() && self.show_moves {
            self.cursor_hand = true;
        }

        if !blocked && self.pointer.left_pressed {
            (self.press_x, self.press_y) = (self.pointer.x, self.pointer.y);
            (self.drag_x, self.drag_y) = (self.pointer.x, self.pointer.y);
            self.press_square = square;
            self.gesture = if self.hover_square.is_some() { Gesture::PressSquare } else { Gesture::TurnCamera };
        }
        if !self.pointer.consumed && self.pointer.right_pressed && self.gesture == Gesture::None {
            self.gesture = Gesture::TurnCamera;
            self.press_square = None;
            (self.press_x, self.press_y) = (self.pointer.x, self.pointer.y);
            (self.drag_x, self.drag_y) = (self.pointer.x, self.pointer.y);
        }

        // A press on a square that moves on turns the view instead.
        let travel = (self.pointer.x - self.press_x).abs() + (self.pointer.y - self.press_y).abs();
        if self.gesture == Gesture::PressSquare && travel > 6.0 * scale && self.pointer.left_down {
            self.gesture = Gesture::TurnCamera;
        }
        if self.gesture == Gesture::TurnCamera {
            self.camera.rotate(-(self.pointer.x - self.drag_x) * 0.0055 / scale, (self.pointer.y - self.drag_y) * 0.0045 / scale);
            (self.drag_x, self.drag_y) = (self.pointer.x, self.pointer.y);
        }

        if self.pointer.left_released || self.pointer.right_released {
            if self.gesture == Gesture::PressSquare && self.pointer.left_released {
                if let (Some(square), true) = (square, square == self.press_square && legal(square)) {
                    self.play_move(square);
                }
            } else if self.gesture == Gesture::TurnCamera && self.pointer.left_released && travel < 4.0 * scale && ready {
                // A click on an empty square where nothing would turn.
                if square.is_some_and(|square| position.at(square).is_none()) {
                    self.show_toast(self.t(Str::NotLegal));
                }
            }
            let left_ends = self.pointer.left_released && self.gesture != Gesture::None && !(self.gesture == Gesture::TurnCamera && self.pointer.right_down);
            let right_ends = self.pointer.right_released && self.gesture == Gesture::TurnCamera && !self.pointer.left_down;
            if left_ends || right_ends {
                self.gesture = Gesture::None;
            }
        }

        if !self.pointer.consumed && self.pointer.wheel != 0.0 {
            self.camera.zoom(self.pointer.wheel);
        }
    }

    fn handle_key(&mut self, key: Key) {
        let control = self.modifiers.control_key();
        let shift = self.modifiers.shift_key();
        let step = if shift { -1 } else { 1 };

        if self.help_visible && matches!(key, Key::Escape | Key::F1) {
            self.help_visible = false;
            return;
        }

        if control {
            match key {
                Key::Character('Z') if shift => self.redo(),
                Key::Character('Z') => self.undo(),
                Key::Character('Y') => self.redo(),
                Key::Character('C') => self.copy_game(shift),
                Key::Character('V') => self.paste_game(),
                Key::Character('N') => self.new_game(),
                _ => {}
            }
            return;
        }

        match key {
            Key::Backspace | Key::Left => self.undo(),
            Key::Right => self.redo(),
            Key::Character('N') => self.new_game(),
            Key::Character('H') => self.request_hint(),
            Key::Character('F') => self.camera.flip(),
            Key::Character('V') => self.camera.toggle_top_view(),
            Key::Home => self.camera.reset(self.view_side()),
            Key::Character('M') => self.set_mode(self.mode.stepped(step)),
            Key::Character('D') => self.toggle_show_moves(),
            Key::Character('T') => self.set_theme(self.theme as i32 + step),
            Key::Character('S') => self.toggle_sound(),
            Key::Character('L') => self.toggle_language(),
            Key::Tab => {
                self.panel_visible = !self.panel_visible;
                self.save();
            }
            Key::F1 => self.help_visible = !self.help_visible,
            Key::F11 => self.toggle_full_screen(),
            Key::Escape if self.full_screen => self.toggle_full_screen(),
            Key::Character(digit @ '1'..='5') => self.set_level(digit as i32 - '0' as i32),
            _ => {}
        }
    }

    // The interface ----------------------------------------------------------------------------------

    /// The rules note in lines that fit the help.
    fn rules_lines(&self) -> Vec<String> {
        let width = (HELP_WIDTH - 2.0 * HELP_PAD) * self.scale;
        let mut lines = Vec::new();
        let mut line = String::new();
        for word in self.t(Str::RulesNote).split(' ') {
            let longer = if line.is_empty() { word.to_owned() } else { format!("{line} {word}") };
            if !line.is_empty() && self.painter.measure(FontId::Body, &longer) > width {
                lines.push(std::mem::replace(&mut line, word.to_owned()));
            } else {
                line = longer;
            }
        }
        if !line.is_empty() {
            lines.push(line);
        }
        lines
    }

    fn help_card(&self) -> Rect {
        let scale = self.scale;
        let rows = (HELP_LINES.len() + 1 + self.rules_lines().len()) as f32;
        let (width, height) = (HELP_WIDTH * scale, (2.0 * HELP_PAD + 44.0 + 12.0 + rows * HELP_ROW) * scale);
        let free = self.free_area();
        rect(free.x + (free.width - width) / 2.0, (10.0 * scale).max((self.height as f32 - height) / 2.0), width, height)
    }

    /// The discs at the end, and why it ended.
    fn result_detail(&self, status: GameStatus) -> String {
        let full = status.black + status.white == SQUARE_COUNT;
        format!("{} : {}  ·  {}", status.black, status.white, self.t(if full { Str::BoardFull } else { Str::NoMovesLeft }))
    }

    fn banner_card(&self) -> Rect {
        let scale = self.scale;
        let title = self.painter.measure(FontId::Display, self.t(result_title(self.banner_status.result))) + 80.0 * scale;
        let detail = self.painter.measure(FontId::Body, &self.result_detail(self.banner_status)) + 60.0 * scale;
        let width = title.max(detail).max(320.0 * scale);
        let free = self.free_area();
        rect(free.x + (free.width - width) / 2.0, 26.0 * scale - (1.0 - self.banner_fade) * 20.0 * scale, width, 92.0 * scale)
    }

    /// Whether a dialog is open over everything else.
    fn modal(&self) -> bool {
        self.help_visible
    }

    fn frosted_panels(&self) -> Vec<FrostedPanel> {
        let mut panels = Vec::new();
        if self.panel_slide > 0.001 {
            panels.push(self.frosted(self.panel_rect(), 18.0, 1.0));
        }
        if self.banner_fade > 0.001 {
            panels.push(self.frosted(self.banner_card(), 18.0, self.banner_fade));
        }
        if self.help_visible {
            panels.push(self.frosted(self.help_card(), 20.0, 1.0));
        }
        panels
    }

    fn player_name(&self, side: Color) -> String {
        if self.mode == Mode::TwoPlayers {
            self.t(Str::Player).to_owned()
        } else if self.is_human(side) {
            self.t(Str::You).to_owned()
        } else {
            format!("{} · {}", self.t(Str::Computer), self.t(Str::level(self.level)))
        }
    }

    fn draw_interface(&mut self) {
        self.draw_panel();
        self.draw_banner();
        self.draw_toasts();
        if self.help_visible {
            self.draw_help();
        }
    }

    fn draw_status(&mut self, area: Rect) {
        let scale = self.scale;
        let status = self.game.status();
        let side = self.game.current().side_to_move();
        let accent = self.accent();
        let thinking = self.computer_thinking || self.computer_result.is_some();
        self.painter.fill(area, 12.0 * scale, rgba(255, 255, 255, 14));

        // A disc of the side to move, or of the winner.
        let shown = match status.result {
            GameResult::WhiteWins => Color::White,
            GameResult::BlackWins => Color::Black,
            _ => side,
        };
        draw_disc(&mut self.painter, area.x + 22.0 * scale, area.y + area.height / 2.0, 10.0 * scale, shown);

        let (title, detail, title_color) = if status.over() {
            (self.t(result_title(status.result)), self.result_detail(status), accent)
        } else {
            let detail = if thinking { self.t(Str::Thinking).to_owned() } else { self.player_name(side) };
            (self.t(if side == Color::Black { Str::BlackToMove } else { Str::WhiteToMove }), detail, TEXT_COLOR)
        };
        let text_x = area.x + 44.0 * scale;
        let line_height = self.painter.line_height(FontId::Heading);
        self.painter.text(FontId::Heading, text_x, area.y + area.height / 2.0 - line_height + 3.0 * scale, title, title_color);
        self.painter.text(FontId::Body, text_x, area.y + area.height / 2.0 + 2.0 * scale, &detail, MUTED_COLOR);
        if thinking || self.hint_thinking {
            self.painter.spinner(area.right() - 22.0 * scale, area.y + area.height / 2.0, 8.0 * scale, 2.2 * scale, self.time * 5.0, accent);
        }
    }

    /// The players with their discs, and a bar shared out between the colors.
    fn draw_score(&mut self, area: Rect) {
        let scale = self.scale;
        let status = self.game.status();
        let row_height = 28.0 * scale;
        let mut y = area.y;
        for side in [Color::Black, Color::White] {
            let text_y = y + (row_height - self.painter.line_height(FontId::Body)) / 2.0;
            let to_move = !status.over() && self.game.current().side_to_move() == side;
            draw_disc(&mut self.painter, area.x + 9.0 * scale, y + row_height / 2.0, 7.0 * scale, side);
            let name = format!("{}  ·  {}", self.t(if side == Color::Black { Str::Black } else { Str::White }), self.player_name(side));
            self.painter.text(FontId::Body, area.x + 26.0 * scale, text_y, &name, if to_move { TEXT_COLOR } else { TEXT_COLOR.fade(0.75) });
            let count = (if side == Color::Black { status.black } else { status.white }).to_string();
            let count_x = area.right() - self.painter.measure(FontId::Heading, &count);
            self.painter.text(FontId::Heading, count_x, y + (row_height - self.painter.line_height(FontId::Heading)) / 2.0, &count, TEXT_COLOR);
            y += row_height;
        }
        y += 6.0 * scale;
        let bar = rect(area.x, y, area.width, 6.0 * scale);
        let share = status.black as f32 / (status.black + status.white).max(1) as f32;
        self.painter.fill(bar, 3.0 * scale, rgba(238, 236, 230, 220));
        self.painter.fill(rect(bar.x, bar.y, bar.height.max(bar.width * share), bar.height), 3.0 * scale, rgba(10, 10, 12, 235));
        self.painter.fill(rect(bar.x + bar.width / 2.0 - 0.5 * scale, bar.y - 2.0 * scale, scale, bar.height + 4.0 * scale), 0.0, rgba(255, 255, 255, 90));
    }

    fn draw_move_list(&mut self, area: Rect) {
        let scale = self.scale;
        self.painter.fill(area, 12.0 * scale, rgba(0, 0, 0, 40));
        let moves = self.game.moves().to_vec();
        if moves.is_empty() {
            self.painter.text_centered(FontId::Body, area, self.t(Str::NoMovesYet), MUTED_COLOR);
            return;
        }

        let row_height = (25.0 * scale).round();
        let inner = area.inset(8.0 * scale);
        let visible = ((inner.height / row_height) as usize).max(1);
        // Each row starts with Black's move.
        let offset = usize::from(self.game.start().side_to_move() == Color::White);
        let rows = (moves.len() + offset).div_ceil(2);
        let max_scroll = rows.saturating_sub(visible) as f32;

        if area.contains(self.pointer.x, self.pointer.y) && self.pointer.wheel != 0.0 && !self.help_visible {
            self.move_scroll = (self.move_scroll - self.pointer.wheel * 3.0).clamp(0.0, max_scroll);
            self.follow_moves = self.move_scroll >= max_scroll;
            self.pointer.wheel = 0.0;
        }
        if self.follow_moves {
            self.move_scroll = max_scroll;
        }
        self.move_scroll = self.move_scroll.clamp(0.0, max_scroll);

        let first = self.move_scroll as usize;
        let number_width = 40.0 * scale;
        let column_width = (inner.width - number_width) / 2.0;
        let accent = self.accent();
        for row in first..rows.min(first + visible) {
            let y = inner.y + (row - first) as f32 * row_height;
            let number = format!("{}.", row + 1);
            let number_x = inner.x + number_width - 10.0 * scale - self.painter.measure(FontId::Body, &number);
            self.painter.text(FontId::Body, number_x, y + 3.0 * scale, &number, MUTED_COLOR);
            for column in 0..2 {
                let Some(ply) = (row * 2 + column).checked_sub(offset).filter(|ply| *ply < moves.len()) else { continue };
                let x = inner.x + number_width + column as f32 * column_width;
                if ply == moves.len() - 1 {
                    self.painter.fill(rect(x - 6.0 * scale, y + scale, column_width - 6.0 * scale, row_height - 2.0 * scale), 7.0 * scale, accent.fade(0.28));
                }
                let record = &moves[ply];
                if record.is_pass() {
                    self.painter.text(FontId::Body, x, y + 3.0 * scale, self.t(Str::Pass), MUTED_COLOR);
                } else {
                    // The square, and how many discs it turned.
                    let width = self.painter.text(FontId::Body, x, y + 3.0 * scale, &square_name(record.square), TEXT_COLOR);
                    let turned = format!("+{}", record.flipped.count_ones());
                    self.painter.text(FontId::Body, x + width + 8.0 * scale, y + 3.0 * scale, &turned, MUTED_COLOR.fade(0.75));
                }
            }
        }
        if max_scroll > 0.0 {
            // A thin scroll bar.
            let bar_height = inner.height * visible as f32 / rows as f32;
            let bar_y = inner.y + (inner.height - bar_height) * (self.move_scroll / max_scroll);
            self.painter.fill(rect(area.right() - 5.0 * scale, bar_y, 3.0 * scale, bar_height), 1.5 * scale, rgba(255, 255, 255, 50));
        }
    }

    fn draw_panel(&mut self) {
        if self.panel_slide <= 0.001 {
            return;
        }
        let scale = self.scale;
        let panel = self.panel_rect();
        if panel.contains(self.pointer.x, self.pointer.y) && (self.pointer.left_pressed || self.pointer.right_pressed) {
            self.pointer.consumed = true;
        }
        self.painter.shadow(panel, 22.0 * scale, rgba(0, 0, 0, 70));
        self.painter.fill(panel, 18.0 * scale, rgba(16, 18, 26, 120));
        self.painter.outline(panel, 18.0 * scale, rgba(255, 255, 255, 24));

        let pad = 20.0 * scale;
        let (x, width) = (panel.x + pad, panel.width - 2.0 * pad);
        let mut y = panel.y + pad;

        self.painter.text(FontId::Heading, x, y, self.t(Str::Title), TEXT_COLOR);
        if self.button(HELP_BUTTON, rect(panel.right() - pad - 30.0 * scale, y - 2.0 * scale, 30.0 * scale, 30.0 * scale), "?", true, false) {
            self.help_visible = !self.help_visible;
        }
        y += 44.0 * scale;

        self.draw_status(rect(x, y, width, 62.0 * scale));
        y += 62.0 * scale + 12.0 * scale;
        self.draw_score(rect(x, y, width, 68.0 * scale));
        y += 68.0 * scale + 14.0 * scale;

        // From the bottom up: the footer, the settings and the buttons; the moves take the space between.
        let mut bottom = panel.bottom() - pad;
        let footer_height = self.painter.line_height(FontId::Body);
        self.painter.text_centered(FontId::Body, rect(x, bottom - footer_height, width, footer_height), self.t(Str::Footer), MUTED_COLOR.fade(0.8));
        bottom -= footer_height + 14.0 * scale;

        let (row_height, gap) = (34.0 * scale, 8.0 * scale);
        let half = (width - gap) / 2.0;
        bottom -= row_height;
        let sound = format!("{}: {}", self.t(Str::Sound), self.t(if self.sound.enabled { Str::On } else { Str::Off }));
        if self.button(SOUND_BUTTON, rect(x, bottom, half, row_height), &sound, true, false) {
            self.toggle_sound();
        }
        if self.button(LANGUAGE_BUTTON, rect(x + half + gap, bottom, half, row_height), self.t(Str::LanguageName), true, false) {
            self.toggle_language();
        }
        bottom -= gap;

        let theme = self.theme();
        let mode_name = [Str::ModeBlack, Str::ModeWhite, Str::ModeTwoPlayers, Str::ModeDemo][self.mode as usize];
        let choices = [
            (THEME_SELECTOR, Str::Theme, (if self.language == Language::Russian { theme.name_russian } else { theme.name_english }).to_owned()),
            (MOVES_SELECTOR, Str::ShowMoves, self.t(if self.show_moves { Str::On } else { Str::Off }).to_owned()),
            (LEVEL_SELECTOR, Str::Level, format!("{} · {}", self.level, self.t(Str::level(self.level)))),
            (MODE_SELECTOR, Str::Mode, self.t(mode_name).to_owned()),
        ];
        for (id, label, value) in choices {
            bottom -= row_height;
            let change = self.selector(id, rect(x, bottom, width, row_height), self.t(label), &value);
            if change != 0 {
                match id {
                    THEME_SELECTOR => self.set_theme(self.theme as i32 + change),
                    MOVES_SELECTOR => self.toggle_show_moves(),
                    LEVEL_SELECTOR => self.set_level(self.level + change),
                    _ => self.set_mode(self.mode.stepped(change)),
                }
            }
            bottom -= 6.0 * scale;
        }
        bottom -= 8.0 * scale;

        let button_height = 38.0 * scale;
        bottom -= button_height;
        if self.button(NEW_GAME_BUTTON, rect(x, bottom, half, button_height), self.t(Str::NewGame), true, false) {
            self.new_game();
        }
        if self.button(FLIP_BUTTON, rect(x + half + gap, bottom, half, button_height), self.t(Str::Flip), true, false) {
            self.camera.flip();
        }
        bottom -= gap + button_height;
        let third = (width - 2.0 * gap) / 3.0;
        if self.button(UNDO_BUTTON, rect(x, bottom, third, button_height), self.t(Str::Undo), self.game.can_undo(), false) {
            self.undo();
        }
        if self.button(REDO_BUTTON, rect(x + third + gap, bottom, third, button_height), self.t(Str::Redo), self.game.can_redo(), false) {
            self.redo();
        }
        let can_hint = self.human_to_move() && !self.hint_thinking;
        if self.button(HINT_BUTTON, rect(x + 2.0 * (third + gap), bottom, third, button_height), self.t(Str::Hint), can_hint, true) {
            self.request_hint();
        }
        bottom -= 16.0 * scale;

        self.painter.text(FontId::Body, x, y, self.t(Str::Moves), MUTED_COLOR);
        y += self.painter.line_height(FontId::Body) + 6.0 * scale;
        if bottom - y > 40.0 * scale {
            self.draw_move_list(rect(x, y, width, bottom - y));
        }
    }

    fn draw_banner(&mut self) {
        if self.banner_fade <= 0.001 {
            return;
        }
        let scale = self.scale;
        let fade = self.banner_fade;
        let status = self.banner_status;
        let card = self.banner_card();
        let detail = self.result_detail(status);
        self.painter.shadow(card, 24.0 * scale, rgba(0, 0, 0, 120).fade(fade));
        self.painter.fill(card, 18.0 * scale, rgba(14, 16, 24, 150).fade(fade));
        self.painter.outline(card, 18.0 * scale, self.accent().fade(0.6 * fade));
        self.painter.text_centered(
            FontId::Display,
            rect(card.x, card.y + 8.0 * scale, card.width, 48.0 * scale),
            self.t(result_title(status.result)),
            TEXT_COLOR.fade(fade),
        );
        self.painter.text_centered(FontId::Body, rect(card.x, card.y + 54.0 * scale, card.width, 26.0 * scale), &detail, MUTED_COLOR.fade(fade));
    }

    fn draw_help(&mut self) {
        let scale = self.scale;
        self.painter.fill(rect(0.0, 0.0, self.width as f32, self.height as f32), 0.0, rgba(0, 0, 0, 110));

        let (row_height, keys_width, pad) = (HELP_ROW * scale, 200.0 * scale, HELP_PAD * scale);
        let card = self.help_card();
        let accent = self.accent();
        self.painter.shadow(card, 26.0 * scale, rgba(0, 0, 0, 140));
        self.painter.fill(card, 20.0 * scale, rgba(18, 20, 28, 175));
        self.painter.outline(card, 20.0 * scale, rgba(255, 255, 255, 30));
        self.painter.text(FontId::Heading, card.x + pad, card.y + pad - 4.0 * scale, self.t(Str::Controls), TEXT_COLOR);
        let mut y = card.y + pad + 44.0 * scale;
        for (named, keys, action) in HELP_LINES {
            self.painter.text(FontId::Body, card.x + pad, y, named.map_or(keys, |id| self.t(id)), accent);
            self.painter.text(FontId::Body, card.x + pad + keys_width, y, self.t(action), TEXT_COLOR);
            y += row_height;
        }
        // The rules, in a few sentences.
        y += 12.0 * scale;
        self.painter.text(FontId::Body, card.x + pad, y, self.t(Str::Rules), accent);
        for line in self.rules_lines() {
            y += row_height;
            self.painter.text(FontId::Body, card.x + pad, y, &line, MUTED_COLOR);
        }
        let close = self.button(HELP_CLOSE, rect(card.right() - pad - 30.0 * scale, card.y + pad - 8.0 * scale, 30.0 * scale, 30.0 * scale), "×", true, false);
        if close || (self.pointer.left_pressed && !card.contains(self.pointer.x, self.pointer.y)) {
            self.help_visible = false;
        }
        if self.pointer.left_pressed || self.pointer.right_pressed {
            self.pointer.consumed = true;
        }
    }
}
