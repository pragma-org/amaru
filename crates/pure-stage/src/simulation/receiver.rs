use tokio::sync::mpsc::{error::TryRecvError, UnboundedReceiver};

/// The message receptacle used by [`SimulationBuilder::output`](super::SimulationBuilder::output).
///
/// It should be noted that [`Self::try_next`] returning `None` only means that the message
/// queue is currently empty — it may be refilled by future simulation steps.
#[derive(Default, Debug)]
pub struct Receiver<T> {
    rx: Option<UnboundedReceiver<T>>,
}

impl<T> Receiver<T> {
    pub fn new(rx: UnboundedReceiver<T>) -> Self {
        Self { rx: Some(rx) }
    }

    /// Extract the next message if there is one.
    pub fn try_next(&mut self) -> Option<T> {
        match self.rx.as_mut()?.try_recv() {
            Ok(msg) => Some(msg),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.rx = None;
                None
            }
        }
    }

    /// Produce an iterator over all messages currently enqueued.
    pub fn drain(&mut self) -> impl Iterator<Item = T> + '_ {
        struct Iter<'a, T>(&'a mut Receiver<T>);
        impl<'a, T> Iterator for Iter<'a, T> {
            type Item = T;

            fn next(&mut self) -> Option<Self::Item> {
                self.0.try_next()
            }
        }
        Iter(self)
    }
}
