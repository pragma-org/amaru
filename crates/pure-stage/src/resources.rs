use parking_lot::{
    MappedRwLockReadGuard, MappedRwLockWriteGuard, RwLock, RwLockReadGuard, RwLockWriteGuard,
};
#[allow(clippy::disallowed_types)]
use std::{
    any::{type_name, Any, TypeId},
    collections::HashMap,
    sync::Arc,
};

/// A collection of resources that can be accessed by external effects.
///
/// This is used to pass resources to external effects while properly scoping the resource to the running stage graph.
/// If you want to share a resource across multiple stage graphs, you can use `Arc<Mutex<T>>` or similar.
///
/// ## API Design Choices
///
/// StageGraph supports single-threaded simulation as well as multi-threaded production code.
/// Since effect implementations must cover both cases with the same code, the resulting API must
/// be constrained by both environments. The simulation can easily provided a `&mut Resources`,
/// but if we require that, then the production code will have to hold a lock on the `Resources`
/// for the whole duration of each effect, serializing all resources.
///
/// Therefore, even though not needed in simulation, we design the API so that the effect can
/// use its resources for shorter durations.
///
/// ## `Sync` Bound
///
/// In order to allow resources to be used without blocking the whole resource collection, shared
/// references can be obtained with read locking. Since this fundamentally allows shared access
/// from multiple threads, the resources must be `Sync`. If your resource is not `Sync`, you can
/// use [`SyncWrapper`](https://docs.rs/sync_wrapper/latest/sync_wrapper/struct.SyncWrapper.html)
/// or a mutex.
#[derive(Default, Clone)]
#[allow(clippy::disallowed_types)]
pub struct Resources(Arc<RwLock<HashMap<TypeId, Box<dyn Any + Send + Sync>>>>);

impl Resources {
    /// Put a resource into the resources collection.
    ///
    /// This variant uses locking to ensure that the resource is not accessed concurrently.
    pub fn put<T: Any + Send + Sync>(&self, resource: T) {
        self.0.write().insert(TypeId::of::<T>(), Box::new(resource));
    }

    /// Get a resource from the resources collection.
    ///
    /// This variant only takes a read lock on the resource collection, allowing other `get`
    /// operations to proceed concurrently. [`get_mut`](Self::get_mut) will be blocked while
    /// the returned guard is held, so [`drop`](std::mem::drop) it as soon as you don't need it
    /// any more.
    pub fn get<T: Any + Send>(&self) -> anyhow::Result<MappedRwLockReadGuard<'_, T>> {
        RwLockReadGuard::try_map(self.0.read(), |res| {
            res.get(&TypeId::of::<T>())?.downcast_ref::<T>()
        })
        .map_err(|_| anyhow::anyhow!("Resource of type `{}` not found", type_name::<T>()))
    }

    /// Get a mutable reference to a resource from the resources collection.
    ///
    /// This variant takes a write lock on the resource collection, blocking all other operations.
    /// See [`get`](Self::get) for a variant that uses read locking. Concurrent operations will
    /// be blocked while the returned guard is held, so [`drop`](std::mem::drop) it as soon as you
    /// don't need it any more.
    ///
    /// If you need exclusive access to a single resource without blocking the rest of the
    /// resource collection, consider putting an `Arc<Mutex<T>>` in the resources collection.
    pub fn get_mut<T: Any + Send>(&mut self) -> anyhow::Result<MappedRwLockWriteGuard<'_, T>> {
        RwLockWriteGuard::try_map(self.0.write(), |res| {
            res.get_mut(&TypeId::of::<T>())?.downcast_mut::<T>()
        })
        .map_err(|_| anyhow::anyhow!("Resource of type `{}` not found", type_name::<T>()))
    }

    /// Take a resource from the resources collection.
    ///
    /// This variant uses locking to ensure that the resource is not accessed concurrently.
    pub fn take<T: Any + Send>(&self) -> anyhow::Result<T> {
        self.0
            .write()
            .remove(&TypeId::of::<T>())
            .ok_or_else(|| anyhow::anyhow!("Resource of type `{}` not found", type_name::<T>()))?
            .downcast::<T>()
            .map(|x| *x)
            .map_err(|_| anyhow::anyhow!("Resource of type `{}` not found", type_name::<T>()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resources() {
        let mut resources = Resources::default();

        assert_eq!(
            resources.get::<u32>().unwrap_err().to_string(),
            "Resource of type `u32` not found"
        );

        resources.put(42u32);
        assert_eq!(*resources.get::<u32>().unwrap(), 42);

        resources.put(43u32);
        assert_eq!(*resources.get_mut::<u32>().unwrap(), 43);

        assert_eq!(resources.take::<u32>().unwrap(), 43);
        assert_eq!(
            resources.take::<u32>().unwrap_err().to_string(),
            "Resource of type `u32` not found"
        );
    }
}
