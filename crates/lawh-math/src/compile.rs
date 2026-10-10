//! One formula from source to [`MathLayout`], on the calling thread. Both the
//! in-process engine and the worker process run this.

use std::sync::Arc;

use typst_layout::PagedDocument;

use crate::world::SandboxWorld;
use crate::{MathError, MathLayout, layout};

/// comemo cache entries unused for this many compiles are evicted, so memory
/// doesn't grow with every formula ever typed.
const COMEMO_MAX_AGE: usize = 10;

/// What one compile produced.
pub(crate) type Outcome = Result<Arc<MathLayout>, MathError>;

/// Compiles and converts one formula, refusing a layout past
/// [`crate::MAX_LAYOUT_EXTENT_PT`].
pub(crate) fn compile(src: &str) -> Outcome {
    let world = SandboxWorld::new(src);
    let compiled = typst::compile::<PagedDocument>(&world).output;
    typst::comemo::evict(COMEMO_MAX_AGE);
    let document = compiled.map_err(|diagnostics| {
        let messages: Vec<&str> = diagnostics.iter().map(|d| d.message.as_str()).collect();
        MathError::Compile(messages.join("; "))
    })?;
    let page = document
        .pages()
        .first()
        .ok_or_else(|| MathError::Compile("the formula produced no page".to_owned()))?;
    let layout = layout::convert(&page.frame)?;
    layout::check_extent(&layout)?;
    Ok(Arc::new(layout))
}

/// Short name of an outcome, for logs.
pub(crate) fn outcome_name(outcome: &Outcome) -> &'static str {
    match outcome {
        Ok(_) => "ok",
        Err(MathError::Compile(_)) => "compile-error",
        Err(MathError::Timeout) => "timeout",
        Err(MathError::Panicked) => "panicked",
        Err(MathError::Crashed) => "crashed",
        Err(MathError::Unavailable(_)) => "unavailable",
        Err(MathError::TooLarge) => "too-large",
    }
}
