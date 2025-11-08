use std::ops::{Deref, DerefMut};

#[derive(Debug, Clone, Default, Eq, serde::Serialize, serde::Deserialize)]
pub struct IgnoreEq<T>(pub T);

impl<T> PartialEq for IgnoreEq<T> {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl<T> From<T> for IgnoreEq<T> {
    fn from(value: T) -> Self {
        IgnoreEq(value)
    }
}

impl<T> Deref for IgnoreEq<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for IgnoreEq<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
