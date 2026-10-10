//! Errors from rendering and export.

/// Why a render or export failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RenderError {
    /// The viewport has no pixels, so there is no image to encode.
    #[error("cannot encode an empty {width}x{height} image")]
    EmptyImage { width: u16, height: u16 },
    /// The PNG encoder failed.
    #[error("could not encode PNG: {0}")]
    Png(String),
}
