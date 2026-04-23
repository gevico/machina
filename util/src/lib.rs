// Clippy 1.94 fires missing_const_for_thread_local even when the
// `const { }` syntax is already present (macro-expansion false positive).
#![allow(clippy::missing_const_for_thread_local)]

pub mod trace;
