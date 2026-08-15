//! SGR escape sequences used when rendering.
//!
//! Written directly rather than through a colour crate: rendering needs exact
//! control over what counts as a visible column (see
//! [`crate::render::display_width`]), and a styling abstraction that could
//! insert sequences of its own would defeat that.

pub const CYAN: &str = "\x1b[36m";
pub const BRIGHT_CYAN: &str = "\x1b[96m";
pub const MAGENTA: &str = "\x1b[95m";
pub const DIM: &str = "\x1b[2m";
pub const BOLD: &str = "\x1b[1m";
pub const YELLOW: &str = "\x1b[33m";
pub const RESET: &str = "\x1b[0m";
