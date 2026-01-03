use std::{
    fmt,
    mem::ManuallyDrop,
    ops::{Deref, DerefMut},
};

pub struct DropGuard<T, F>
where
    F: FnOnce(T),
{
    inner: ManuallyDrop<T>,
    f: ManuallyDrop<F>,
}

impl<T, F: FnOnce(T)> DropGuard<T, F> {
    pub const fn new(inner: T, f: F) -> Self {
        Self {
            inner: ManuallyDrop::new(inner),
            f: ManuallyDrop::new(f),
        }
    }

    pub fn into_inner(guard: Self) -> T {
        let mut guard = ManuallyDrop::new(guard);
        let value = unsafe { ManuallyDrop::take(&mut guard.inner) };
        unsafe { ManuallyDrop::drop(&mut guard.f) };
        value
    }
}

impl<T, F: FnOnce(T)> Deref for DropGuard<T, F> {
    type Target = T;

    fn deref(&self) -> &T {
        &*self.inner
    }
}

impl<T, F: FnOnce(T)> DerefMut for DropGuard<T, F> {
    fn deref_mut(&mut self) -> &mut T {
        &mut *self.inner
    }
}

impl<T, F: FnOnce(T)> Drop for DropGuard<T, F> {
    fn drop(&mut self) {
        let inner = unsafe { ManuallyDrop::take(&mut self.inner) };
        let f = unsafe { ManuallyDrop::take(&mut self.f) };
        f(inner);
    }
}

impl<T: fmt::Debug, F: FnOnce(T)> fmt::Debug for DropGuard<T, F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}
