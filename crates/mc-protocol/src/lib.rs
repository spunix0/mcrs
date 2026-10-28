//! Java Edition network protocol for the pinned version (26.3): packet types, VarInt framing, compression and encryption, login/configuration/play states.

pub mod buf;
pub mod conn;
pub mod ids;
pub mod text;

pub use buf::{Reader, Writer};
pub use conn::{Receiver, Sender, connect};

pub const PROTOCOL_VERSION: i32 = 777;
pub const GAME_VERSION: &str = "26.3";
