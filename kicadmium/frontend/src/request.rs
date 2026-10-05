//! Cancellation token for component-scoped asynchronous work.
//!
//! Browser fetches cannot always be aborted portably, but their results must
//! never be committed after an effect has been replaced or unmounted.

use std::{cell::Cell, rc::Rc};

#[derive(Clone, Default)]
pub struct RequestToken(Rc<Cell<bool>>);

impl RequestToken {
    pub fn current(&self) -> bool {
        !self.0.get()
    }

    pub fn cancel(&self) {
        self.0.set(true);
    }
}

#[cfg(test)]
mod tests {
    use super::RequestToken;

    #[test]
    fn cancellation_is_shared_with_in_flight_task() {
        let effect = RequestToken::default();
        let task = effect.clone();
        assert!(task.current());
        effect.cancel();
        assert!(!task.current());
    }
}
