//! saysdfTerm: shared logic of the `saysdf` TUI utilities (no terminal I/O).
//!
//! - [`sanitize`] — visible symbols instead of control characters, tab expansion, width;
//! - [`source`] — file loading: binary detection, lines, large files, hex view;
//! - [`highlight`] — syntect highlighting styled from the terminal palette;
//! - [`doc`] — pager document model and search;
//! - [`markdown`] — Markdown: block model, width layout, images;
//! - [`mermaid`] — Mermaid as character graphics (flowchart, sequence);
//! - [`diff`] — diff model, `git diff` parsing, hunk patch, rendering;
//! - [`merge`] — conflict markers, three-way merge.

pub mod diff;
pub mod doc;
pub mod highlight;
pub mod markdown;
pub mod merge;
pub mod mermaid;
pub mod sanitize;
pub mod source;
