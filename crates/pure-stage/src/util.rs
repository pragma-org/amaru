#![allow(dead_code)]

/// Helper type to wrap futures/functions/etc. and thus avoid having to handroll
/// a `Debug` implementation for a type containing the wrapped value.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct NoDebug<T>(T);
impl<T> NoDebug<T> {
    pub fn new(t: T) -> Self {
        Self(t)
    }

    pub fn into_inner(self) -> T {
        self.0
    }
}
impl<T> std::ops::Deref for NoDebug<T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl<T> std::ops::DerefMut for NoDebug<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
impl<T> std::fmt::Debug for NoDebug<T> {
    fn fmt(&self, _f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Ok(())
    }
}

/// A type that is not inhabited.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Void {}
