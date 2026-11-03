//! What the scene reader needs of the drawing code.

pub type P = (f64, f64);

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Fill {
    Hachure,
    CrossHatch,
    Solid,
    Zigzag,
}
