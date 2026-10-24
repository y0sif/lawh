//! What an export asks for and what it reports: the data `:export` and `lawh export` hand the
//! renderer, which draws the board into the file's bytes (`lawh-render`), and what comes back.
//! No rendering here.

/// A file format a board exports to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExportFormat {
    Svg,
    Png,
    Pdf,
}

impl ExportFormat {
    /// The format a path's extension names, in any case: `.svg`, `.png` or `.pdf`. Anything
    /// else, no extension, or a name that is only the extension (`.png`, a hidden file), is
    /// `None`.
    pub fn from_path(path: &str) -> Option<Self> {
        // The last component, after either separator: a dot in a directory's name is no
        // extension.
        let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
        let (stem, extension) = name.rsplit_once('.')?;
        if stem.is_empty() {
            return None;
        }
        [Self::Svg, Self::Png, Self::Pdf]
            .into_iter()
            .find(|format| extension.eq_ignore_ascii_case(format.extension()))
    }

    /// The format's extension, lowercase and without the dot: `"svg"`, `"png"` or `"pdf"`.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Svg => "svg",
            Self::Png => "png",
            Self::Pdf => "pdf",
        }
    }
}

/// What is drawn behind the board's objects (`:set exportbg`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ExportBackground {
    /// The board's paper colour.
    #[default]
    Paper,
    /// Nothing: a transparent PNG or SVG, a PDF page with no fill.
    None,
}

/// How to export a board.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ExportOptions {
    pub format: ExportFormat,
    pub background: ExportBackground,
    /// The PNG's scale in hundredths (`:set pngscale`, 0.25 to 8): pixels per grid unit are
    /// the renderer's 40 times this over 100. From [`MIN_PNG_SCALE_HUNDREDTHS`] to
    /// [`MAX_PNG_SCALE_HUNDREDTHS`]; the renderer keeps a value outside within them.
    pub png_scale_hundredths: u32,
}

impl Default for ExportOptions {
    /// A PNG on paper at the default scale.
    fn default() -> Self {
        Self {
            format: ExportFormat::Png,
            background: ExportBackground::Paper,
            png_scale_hundredths: DEFAULT_PNG_SCALE_HUNDREDTHS,
        }
    }
}

/// What an export that wrote its file has to say about it.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ExportReport {
    /// Formulas that failed to compile or took too long on their own: drawn as on screen, their
    /// source in the error colour.
    pub failed_formulas: usize,
    /// The factor the scale was shrunk by to fit a size cap, in (0, 1); `None` at full scale.
    pub shrunk: Option<f64>,
}

/// The PNG scale `:set pngscale` starts at, in hundredths: 2, 80 pixels a grid unit, crisp on a
/// screen with twice the pixels.
pub const DEFAULT_PNG_SCALE_HUNDREDTHS: u32 = 200;

/// The smallest PNG scale, in hundredths: 0.25, 10 pixels a grid unit, where text is about the
/// smallest still readable (an em of 5 pixels).
pub const MIN_PNG_SCALE_HUNDREDTHS: u32 = 25;

/// The largest PNG scale, in hundredths: 8, 320 pixels a grid unit. Larger boards are shrunk to
/// the renderer's pixel caps anyway, and print resolution is reached well before.
pub const MAX_PNG_SCALE_HUNDREDTHS: u32 = 800;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_come_from_the_extension_in_any_case() {
        for (path, format) in [
            ("out.svg", ExportFormat::Svg),
            ("out.png", ExportFormat::Png),
            ("out.pdf", ExportFormat::Pdf),
            ("OUT.PNG", ExportFormat::Png),
            ("board.Pdf", ExportFormat::Pdf),
            ("~/notes/area.sVg", ExportFormat::Svg),
            ("a.b.png", ExportFormat::Png),
            ("dir.pdf/out.png", ExportFormat::Png),
            ("C:\\boards\\out.PDF", ExportFormat::Pdf),
            ("with space.png", ExportFormat::Png),
        ] {
            assert_eq!(ExportFormat::from_path(path), Some(format), "{path}");
        }
    }

    #[test]
    fn other_or_missing_extensions_are_no_format() {
        for path in [
            "",
            "out",
            "out.lawh",
            "out.jpg",
            "out.png.lawh",
            "out.",
            ".png",
            "dir/.pdf",
            "dir.png/",
            "dir.png/out",
            "out.pngx",
            "out. png",
            "out.p\u{0144}g",
        ] {
            assert_eq!(ExportFormat::from_path(path), None, "{path:?}");
        }
    }

    #[test]
    fn extensions_name_their_format_again() {
        for format in [ExportFormat::Svg, ExportFormat::Png, ExportFormat::Pdf] {
            let path = format!("x.{}", format.extension());
            assert_eq!(ExportFormat::from_path(&path), Some(format));
            assert_eq!(format.extension(), format.extension().to_lowercase());
        }
    }

    #[test]
    fn options_default_to_a_png_on_paper_at_scale_2() {
        let options = ExportOptions::default();
        assert_eq!(options.format, ExportFormat::Png);
        assert_eq!(options.background, ExportBackground::Paper);
        assert_eq!(options.png_scale_hundredths, 200);
        assert!(
            (MIN_PNG_SCALE_HUNDREDTHS..=MAX_PNG_SCALE_HUNDREDTHS)
                .contains(&DEFAULT_PNG_SCALE_HUNDREDTHS)
        );
        assert_eq!(ExportReport::default().failed_formulas, 0);
        assert_eq!(ExportReport::default().shrunk, None);
    }
}
