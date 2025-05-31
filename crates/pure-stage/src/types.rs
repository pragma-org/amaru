use std::{any::Any, borrow::Borrow, fmt, future::Future, pin::Pin, sync::Arc};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Type constraint for messages, which must be self-contained and have a `Debug` instance.
///
/// It is not possible to require an implementation of `PartialEq<Box<dyn Message>>`, but it
/// is possible to provide a blanket implementation for an equivalent `eq` method, which can
/// be used to manually implement PartialEq for types containing messages.
///
/// See [`cast_msg`](cast_msg) for a utility function casting a generic message to a concrete type.
pub trait Message: Any + fmt::Debug + Send + 'static {
    /// Check for equality with another dynamically typed message.
    ///
    /// This is useful for implementing `PartialEq` for types containing boxed messages.
    fn eq(&self, other: &dyn Message) -> bool;

    /// Get the type name of the message given a reference to a message.
    ///
    /// When the type is statically known, use `std::any::type_name::<T>()` instead.
    fn type_name(&self) -> &'static str;
}
impl<T: Any + fmt::Debug + Send + PartialEq + 'static> Message for T {
    fn eq(&self, other: &dyn Message) -> bool {
        let Some(other) = (other as &dyn Any).downcast_ref::<T>() else {
            return false;
        };
        self == other
    }
    fn type_name(&self) -> &'static str {
        std::any::type_name::<T>()
    }
}

impl dyn Message {
    /// Cast a message to a given concrete type.
    pub fn cast_ref<T: Message>(&self) -> Option<&T> {
        (self as &dyn Any).downcast_ref::<T>()
    }

    /// Cast a message to a given concrete type, yielding an informative error otherwise
    pub fn cast<T: Message>(self: Box<Self>) -> anyhow::Result<Box<T>> {
        if (&*self as &dyn Any).is::<T>() {
            #[allow(clippy::expect_used)]
            Ok(Box::new(
                *(self as Box<dyn Any>)
                    .downcast::<T>()
                    .expect("checked above"),
            ))
        } else {
            anyhow::bail!(
                "message type error: expected {}, got {:?} ({})",
                std::any::type_name::<T>(),
                self,
                (*self).type_name()
            )
        }
    }
}

impl PartialEq for dyn Message {
    fn eq(&self, other: &Self) -> bool {
        self.type_id() == other.type_id() && Message::eq(self, other)
    }
}

/// Type constraint for messages, which must be self-contained and have a `Debug` instance.
///
/// It is not possible to require an implementation of `PartialEq<Box<dyn Message>>`, but it
/// is possible to provide a blanket implementation for an equivalent `eq` method, which can
/// be used to manually implement PartialEq for types containing messages.
///
/// See [`cast_state`](cast_state) for a utility function casting a generic message to a concrete type.
pub trait State: Any + fmt::Debug + Send + 'static {
    fn type_name(&self) -> &str;
}
impl<T: Any + fmt::Debug + Send + 'static> State for T {
    fn type_name(&self) -> &str {
        std::any::type_name::<T>()
    }
}

impl dyn State {
    /// Cast a state to a given concrete type.
    pub fn cast_ref<T: State>(&self) -> Option<&T> {
        (self as &dyn Any).downcast_ref::<T>()
    }

    /// Cast a state to a given concrete type, yielding an informative error otherwise
    pub fn cast<T: State>(self: Box<Self>) -> anyhow::Result<Box<T>> {
        if (&*self as &dyn Any).is::<T>() {
            #[allow(clippy::expect_used)]
            Ok(Box::new(
                *(self as Box<dyn Any>)
                    .downcast::<T>()
                    .expect("checked above"),
            ))
        } else {
            anyhow::bail!(
                "state type error: expected {}, got {:?} ({})",
                std::any::type_name::<T>(),
                self,
                (*self).type_name()
            )
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Name(Arc<str>);

impl Name {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn append(&self, other: &str) -> Self {
        let mut new = String::with_capacity(self.0.len() + other.len());
        new.push_str(&self.0);
        new.push_str(other);
        Self(new.into())
    }
}

impl AsRef<str> for Name {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Borrow<str> for Name {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl From<&str> for Name {
    fn from(value: &str) -> Self {
        Self(value.into())
    }
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod test {
    use crate::{Message, State};

    #[test]
    fn message() {
        let s = Box::new("hello".to_owned()) as Box<dyn Message>;
        assert_eq!(format!("{s:?}"), "\"hello\"");
        assert_eq!(
            s.cast::<&'static str>().unwrap_err().to_string(),
            "message type error: expected &str, got \"hello\" (alloc::string::String)"
        );

        let s = Box::new("hello".to_owned()) as Box<dyn Message>;
        assert_eq!(*s.cast::<String>().unwrap(), "hello");

        // the following tests show that this type of cast is robust regarding
        // auto-dereferencing, which is a common source of confusion when using
        // trait objects.

        let r0 = 1u32;
        let r1: &dyn Message = &r0;
        let r2 = &r1;
        let r3 = &r2;

        assert_eq!(r1.cast_ref::<u32>().unwrap(), &1);
        assert_eq!(r2.cast_ref::<u32>().unwrap(), &1);
        assert_eq!(r3.cast_ref::<u32>().unwrap(), &1);

        let r0: Box<dyn Message> = Box::new(1u32);
        let r1 = &r0;
        let r2 = &r1;
        let r3 = &r2;

        assert_eq!(r0.cast_ref::<u32>().unwrap(), &1);
        assert_eq!(r1.cast_ref::<u32>().unwrap(), &1);
        assert_eq!(r2.cast_ref::<u32>().unwrap(), &1);
        assert_eq!(r3.cast_ref::<u32>().unwrap(), &1);
    }

    #[test]
    fn state() {
        let s = Box::new("hello".to_owned()) as Box<dyn State>;
        assert_eq!(format!("{s:?}"), "\"hello\"");
        assert_eq!(
            s.cast::<&'static str>().unwrap_err().to_string(),
            "state type error: expected &str, got \"hello\" (alloc::string::String)"
        );

        let s = Box::new("hello".to_owned()) as Box<dyn State>;
        assert_eq!(*s.cast::<String>().unwrap(), "hello");
    }
}
