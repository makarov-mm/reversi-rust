//! 3D Reversi: a Reversi (Othello) game on OpenGL 4.6.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod board_geometry;
mod board_view;
mod camera;
mod engine;
mod gfx;
mod math;
mod piece_models;
mod platform;
mod renderer;
mod reversi;
mod settings;
mod shaders;
mod sound;
mod strings;
mod text;
mod themes;
mod ui;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn main() {
    if let Err(error) = app::run() {
        platform::show_error("3D Reversi", &error.to_string());
        std::process::exit(1);
    }
}
