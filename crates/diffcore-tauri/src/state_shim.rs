//! Drop-in replacement for `tauri::State` when the `desktop` feature is off,
//! so command signatures stay identical in web builds.

use std::ops::Deref;

pub struct State<'a, T: ?Sized>(pub &'a T);

impl<'a, T: ?Sized> Deref for State<'a, T> {
    type Target = T;

    fn deref(&self) -> &T {
        self.0
    }
}
