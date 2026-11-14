//! UI-agnostic avatar engine: model, rig, renderer, export, app state.
//!
//! `no_std` + `alloc`, no dependencies, so it links into the AerOS kernel and
//! also builds on the host. A frontend only needs `state::AppState`: send it
//! `Command`s, read back `AppState::render` frames (RGBA8) and the catalogs
//! in `catalog`.
#![no_std]

extern crate alloc;

pub mod animate;
pub mod catalog;
pub mod error;
pub mod export;
pub mod fmath;
pub mod gfx;
pub mod model;
pub mod render;
pub mod rig;
pub mod state;
pub mod tracking;
pub mod ui;

pub use error::{Error, Result};
pub use model::Avatar;
pub use rig::{Expression, Pose};
pub use state::{AppState, Command};
