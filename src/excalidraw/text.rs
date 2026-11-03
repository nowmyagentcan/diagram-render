//! Font metrics of Excalidraw 0.18.1 (`fonts/FontMetadata.ts`, MIT).

/// Units per em, ascender, descender and line height of a font id; Virgil's for other ids, as
/// Excalidraw falls back to them.
pub fn metrics(id: i64) -> (f64, f64, f64, f64) {
    match id {
        2 => (2048.0, 1577.0, -471.0, 1.15),
        3 => (2048.0, 1900.0, -480.0, 1.2),
        6 => (1000.0, 1011.0, -353.0, 1.35),
        7 => (1000.0, 923.0, -220.0, 1.15),
        8 => (1000.0, 750.0, -250.0, 1.25),
        9 => (2048.0, 1854.0, -434.0, 1.15),
        _ => (1000.0, 886.0, -374.0, 1.25),
    }
}
