//! Multi-client rendering preview engine (FR-RENDER-01..40).
//!
//! Three concerns live here:
//!
//!   - [`profile`] — client profile transforms (Gmail Web,
//!     Outlook, Apple Mail, …) that rewrite HTML to surface each
//!     client's quirks.
//!   - [`lint`] — HTML linter that flags known-bad patterns
//!     (style-in-body, CSS Grid, web fonts in Outlook, etc.).
//!   - [`a11y`] — accessibility linter: contrast, alt text,
//!     "click here" detection, heading semantics.
//!
//! All output is honest-by-design: each profile carries a fidelity
//! badge so we never claim "this is exactly Outlook 2019". The
//! reality is we're an approximation; the badges are how we say so.

pub mod a11y;
pub mod lint;
pub mod profile;
