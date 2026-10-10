//! The Typst sandbox: one in-memory source, bundled fonts, no files, no
//! packages, no clock, no WebAssembly plugins, no image decoding, no data
//! loading and no `raw` by name.

use std::sync::OnceLock;

use typst::diag::{FileError, FileResult};
use typst::foundations::{Binding, Bytes, Datetime, Duration, Module, Scope};
use typst::syntax::{FileId, Source};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Library, LibraryExt, World};

use crate::MATH_FONT_SIZE_PT;
use crate::fonts::{MATH_FAMILY, TEXT_FAMILY, typst_fonts};

/// Standard library definitions a board must not reach. A formula never
/// needs any of them.
///
/// - `plugin` loads and runs WebAssembly: code we never want a shared file to
///   execute.
/// - `cite` and `bibliography` parse CSL styles given as inline bytes with
///   quick-xml 0.38, which has open denial-of-service advisories
///   (RUSTSEC-2026-0194, RUSTSEC-2026-0195).
/// - `image` decodes PNG, JPEG, GIF, WebP and SVG from inline bytes: a small
///   PNG can claim 40000x40000 pixels and take gigabytes to decode.
/// - `read` and the data loaders `json`, `yaml`, `toml`, `cbor`, `csv` and
///   `xml` read files (denied anyway) or parse inline bytes; deep XML
///   overflows the stack and a CBOR header can claim a huge array.
/// - `raw` parses a `theme` given as inline bytes as a plist (XML, through
///   quick-xml) and `syntaxes` as YAML (yaml-rust). It never draws in lawh
///   anyway: there is no bundled monospace font, so any raw text fails with
///   "no font could be found".
///
///   This only narrows the way in. The element function is still reachable
///   from raw content, which backticks in a content block or `eval` make
///   without naming it (`` #let f = [`x`].func(); #f("y", theme: bytes(..)) ``),
///   so those parsers can still run. They run in the worker process, under
///   its memory cap and timeout, like the rest of Typst.
const DENIED: &[&str] = &[
    "plugin",
    "cite",
    "bibliography",
    "image",
    "read",
    "json",
    "yaml",
    "toml",
    "cbor",
    "csv",
    "xml",
    "raw",
];

/// Value of the metadata element that marks the first line's baseline.
///
/// Typst flattens the equation into the page frame and drops the line
/// baselines, but a tag (which metadata produces) is placed on its row's
/// baseline, and math spacing ignores tags, so the marker doesn't move
/// anything.
pub(crate) const BASELINE_MARKER: &str = "lawh-baseline";

/// Wraps user math in the document lawh compiles: display math on a page that
/// shrinks to the formula, with no margin and no background.
///
/// The source sits on its own lines so a trailing `//` comment can't swallow
/// the closing `$`.
pub(crate) fn document(src: &str) -> String {
    format!(
        "#set page(width: auto, height: auto, margin: 0pt, fill: none)\n\
         #set text(font: \"{TEXT_FAMILY}\", size: {MATH_FONT_SIZE_PT}pt)\n\
         #show math.equation: set text(font: \"{MATH_FAMILY}\")\n\
         $\n#metadata(\"{BASELINE_MARKER}\")\n{src}\n$\n"
    )
}

/// A Typst world that can see exactly one file: the generated document.
pub(crate) struct SandboxWorld {
    main: Source,
}

impl SandboxWorld {
    pub(crate) fn new(src: &str) -> Self {
        Self::from_document(document(src))
    }

    pub(crate) fn from_document(text: String) -> Self {
        Self {
            main: Source::detached(text),
        }
    }
}

impl World for SandboxWorld {
    fn library(&self) -> &LazyHash<Library> {
        static LIBRARY: OnceLock<LazyHash<Library>> = OnceLock::new();
        LIBRARY.get_or_init(|| LazyHash::new(sandboxed_library()))
    }

    fn book(&self) -> &LazyHash<FontBook> {
        static BOOK: OnceLock<LazyHash<FontBook>> = OnceLock::new();
        BOOK.get_or_init(|| LazyHash::new(FontBook::from_fonts(typst_fonts())))
    }

    fn main(&self) -> FileId {
        self.main.id()
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        if id == self.main.id() {
            Ok(self.main.clone())
        } else {
            Err(FileError::AccessDenied)
        }
    }

    fn file(&self, _id: FileId) -> FileResult<Bytes> {
        Err(FileError::AccessDenied)
    }

    fn font(&self, index: usize) -> Option<Font> {
        typst_fonts().get(index).cloned()
    }

    fn today(&self, _offset: Option<Duration>) -> Option<Datetime> {
        None
    }
}

/// The default standard library with the [`DENIED`] definitions removed, both
/// from the global scope and from the `std` module that mirrors it.
fn sandboxed_library() -> Library {
    let mut library = Library::default();
    let mut scope = Scope::new();
    for (name, binding) in library.global.scope().iter() {
        if !DENIED.contains(&name.as_str()) {
            scope.bind(name.clone(), binding.clone());
        }
    }
    library.global = Module::new("global", scope);
    library.std = Binding::detached(library.global.clone());
    library
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn denied_names_are_gone_from_global_and_std() {
        let library = sandboxed_library();
        for name in DENIED {
            assert!(
                library.global.scope().get(name).is_none(),
                "{name} in global"
            );
            let typst::foundations::Value::Module(std) = library.std.read() else {
                panic!("std is not a module");
            };
            assert!(std.scope().get(name).is_none(), "{name} in std");
        }
        // Everything else is still there.
        assert!(library.global.scope().get("range").is_some());
        assert!(library.global.scope().get("math").is_some());
    }

    #[test]
    fn only_the_main_source_is_readable() {
        let world = SandboxWorld::new("x");
        assert!(world.source(world.main()).is_ok());
        assert!(world.file(world.main()).is_err());
        assert!(world.today(None).is_none());
    }
}
