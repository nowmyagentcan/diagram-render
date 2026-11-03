//! Excalidraw scenes (`.excalidraw`, `.excalidraw.svg`, `.excalidraw.png`).

#[allow(dead_code)] // the drawing code reads the rest
mod read;
#[allow(dead_code)]
mod rough;
mod text;

use crate::engine::{Checked, Rendered, Task};
use crate::error::Error;

pub fn render(task: &Task) -> Result<Rendered, Error> {
    read::scene(&task.lines, task.name)?;
    Err(Error::internal("no renderer is built in yet"))
}

/// What reading the scene finds.
pub fn check(task: &Task) -> Result<Checked, Error> {
    Ok(Checked {
        warnings: read::scene(&task.lines, task.name)?.warnings,
        ..Checked::default()
    })
}
