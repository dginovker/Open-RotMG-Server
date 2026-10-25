use serde::Serialize;
use std::collections::BTreeSet;

/// An entity field that a world index must follow; outside this module it can only change through `set`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
#[serde(transparent)]
pub struct Tracked<T>(Option<T>);

impl<T> Tracked<T> {
    pub fn set(&mut self, value: Option<T>, id: i32, index: &mut BTreeSet<i32>) {
        if value.is_some() {
            index.insert(id);
        }
        self.0 = value;
    }
}

impl<T> std::ops::Deref for Tracked<T> {
    type Target = Option<T>;
    fn deref(&self) -> &Option<T> {
        &self.0
    }
}

impl<T: PartialEq> PartialEq<Option<T>> for Tracked<T> {
    fn eq(&self, other: &Option<T>) -> bool {
        self.0 == *other
    }
}
