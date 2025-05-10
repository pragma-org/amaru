use std::{any::Any, fmt, future::Future, pin::Pin};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Type constraint for messages, which must be self-contained and have a `Debug` instance.
///
/// It is not possible to require an implementation of `PartialEq<Box<dyn Message>>`, but it
/// is possible to provide a blanket implementation for an equivalent `eq` method, which can
/// be used to manually implement PartialEq for types containing messages.
///
/// See [`cast_msg`](cast_msg) for a utility function casting a generic message to a concrete type.
pub trait Message: Any + fmt::Debug + Send + 'static {
    fn eq(&self, other: &dyn Message) -> bool;
    fn type_name(&self) -> &str;
}
impl<T: Any + fmt::Debug + Send + PartialEq + 'static> Message for T {
    fn eq(&self, other: &dyn Message) -> bool {
        let Some(other) = (other as &dyn Any).downcast_ref::<T>() else {
            return false;
        };
        self == other
    }
    fn type_name(&self) -> &str {
        std::any::type_name::<T>()
    }
}

/// Cast a message to a given concrete type, yielding an informative error otherwise
pub fn cast_msg<T: Message>(this: Box<dyn Message>) -> anyhow::Result<T> {
    // we could get rid of this .is/.downcast duplication only if we don't print
    // the actual Debug representation of the message; since this is only used
    // in testing, it is probably okay
    if (&*this as &dyn Any).is::<T>() {
        #[allow(clippy::expect_used)]
        Ok(*(this as Box<dyn Any>)
            .downcast::<T>()
            .expect("checked above"))
    } else {
        anyhow::bail!(
            "message type error: expected {}, got {:?} ({})",
            std::any::type_name::<T>(),
            this,
            (*this).type_name()
        )
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

/// Cast a state to a given concrete type, yielding an informative error otherwise
///
/// NOTE that Rust coercion rules for trait references mean that you can pass
/// `&Box<dyn State>`, which will compile fine but it won't work because then
/// the original type of the State will be erased.
pub fn cast_state<T: State>(this: &dyn State) -> anyhow::Result<&T> {
    // we could get rid of this .is/.downcast duplication only if we don't print
    // the actual Debug representation of the message; since this is only used
    // in testing, it is probably okay
    if (this as &dyn Any).is::<T>() {
        #[allow(clippy::expect_used)]
        Ok((this as &dyn Any)
            .downcast_ref::<T>()
            .expect("checked above"))
    } else {
        anyhow::bail!(
            "state type error: expected {}, got {:?} ({})",
            std::any::type_name::<T>(),
            this,
            this.type_name()
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Name(String);

impl Name {
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl AsRef<str> for Name {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl From<&str> for Name {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod test {
    use super::Message;
    use crate::{cast_msg, cast_state, State};

    #[test]
    fn message() {
        let s = Box::new("hello".to_owned()) as Box<dyn Message>;
        assert_eq!(format!("{s:?}"), "\"hello\"");
        assert_eq!(
            cast_msg::<&'static str>(s).unwrap_err().to_string(),
            "message type error: expected &str, got \"hello\" (alloc::string::String)"
        );

        let s = Box::new("hello".to_owned()) as Box<dyn Message>;
        assert_eq!(cast_msg::<String>(s).unwrap(), "hello");
    }

    #[test]
    fn state() {
        let s = Box::new("hello".to_owned()) as Box<dyn State>;
        assert_eq!(format!("{s:?}"), "\"hello\"");
        assert_eq!(
            cast_state::<&'static str>(&*s).unwrap_err().to_string(),
            "state type error: expected &str, got \"hello\" (alloc::string::String)"
        );

        let s = Box::new("hello".to_owned()) as Box<dyn State>;
        assert_eq!(cast_state::<String>(&*s).unwrap(), "hello");
    }
}
