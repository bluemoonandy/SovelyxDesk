// Exercise the production geometry on non-Windows hosts without native dependencies.
// rustc --edition=2024 --test -D warnings scripts/test_display_geometry.rs -o <test binary>
#[allow(dead_code)]
mod error {
    #[derive(Debug)]
    pub enum Error {
        Protocol(String),
        Display(String),
    }
    pub type Result<T> = std::result::Result<T, Error>;
}
#[path = "../src/geometry.rs"]
pub mod geometry;
