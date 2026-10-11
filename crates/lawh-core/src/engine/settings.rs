//! Settings that change how the engine reads keys.

/// Where `<Esc>` goes from Insert mode in a text block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Escape {
    /// Back to where Insert mode was entered from: the board (with the block as the current
    /// object) or block-Normal mode.
    #[default]
    Board,
    /// Always to block-Normal mode; `<Esc>` there goes on to the board.
    Block,
}

/// The engine's settings. [`Engine::settings_mut`](super::Engine::settings_mut) changes them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    pub escape: Escape,
}
