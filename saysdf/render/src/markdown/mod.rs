//! Markdown preview (change markdown-preview): parsing into a block model ([`parse`]), layout by
//! width ([`layout`]), images ([`image`], local only).

mod critic;
pub mod image;
pub mod layout;
pub mod model;
pub mod parse;

pub use layout::{layout, HeadingRef, ImagePlace, Layout, LayoutOptions, LinkRef, MermaidRenderer};
pub use model::{slug, Align, Block, BlockKind, CriticPart, Doc, Inline, Item};
pub use parse::parse;

#[cfg(test)]
mod tests;
