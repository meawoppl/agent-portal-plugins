//! Keep a multi-pointer navigation gesture from selecting an item on release.
//!
//! The shared input engine tracks each finger's travel independently. A finger
//! held still while another pinches can therefore look like a tap when it lifts
//! last. Remember the whole gesture, until every pointer has left the canvas.

use vector_view::{
    input::{Input, InputEvent, InputOutcome},
    view::View,
};

#[derive(Default)]
pub struct ViewInput {
    input: Input,
    multi_pointer: bool,
}

impl ViewInput {
    pub fn handle(&mut self, event: &InputEvent, view: &mut View) -> InputOutcome {
        if matches!(event, InputEvent::PointerDown { .. }) && self.input.active_pointers() == 0 {
            self.multi_pointer = false;
        }
        let outcome = self.input.handle(event, view);
        self.multi_pointer |= self.input.active_pointers() > 1;
        if self.multi_pointer && matches!(outcome, InputOutcome::Tap { .. }) {
            InputOutcome::None
        } else {
            outcome
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn down(id: i32, x: f64) -> InputEvent {
        InputEvent::PointerDown {
            id,
            x,
            y: 100.0,
            button: 0,
            time_ms: 0.0,
        }
    }
    fn up(id: i32, x: f64) -> InputEvent {
        InputEvent::PointerUp {
            id,
            x,
            y: 100.0,
            button: 0,
            time_ms: 100.0,
        }
    }

    #[test]
    fn stationary_pinch_finger_does_not_select_and_next_tap_still_works() {
        let mut input = ViewInput::default();
        let mut view = View::default();
        input.handle(&down(1, 100.0), &mut view);
        input.handle(&down(2, 200.0), &mut view);
        let scale = view.scale;
        input.handle(
            &InputEvent::PointerMove {
                id: 2,
                x: 250.0,
                y: 100.0,
            },
            &mut view,
        );
        assert!(view.scale > scale);
        assert_eq!(input.handle(&up(2, 250.0), &mut view), InputOutcome::None);
        assert_eq!(input.handle(&up(1, 100.0), &mut view), InputOutcome::None);
        input.handle(&down(3, 100.0), &mut view);
        assert!(matches!(
            input.handle(&up(3, 100.0), &mut view),
            InputOutcome::Tap { .. }
        ));
    }

    #[test]
    fn cancelled_second_finger_does_not_turn_remaining_finger_into_a_tap() {
        let mut input = ViewInput::default();
        let mut view = View::default();
        input.handle(&down(1, 100.0), &mut view);
        input.handle(&down(2, 200.0), &mut view);
        input.handle(&InputEvent::PointerCancel { id: 2 }, &mut view);
        assert_eq!(input.handle(&up(1, 100.0), &mut view), InputOutcome::None);
        input.handle(&down(3, 100.0), &mut view);
        input.handle(&InputEvent::PointerCancel { id: 3 }, &mut view);
        let before = view;
        input.handle(
            &InputEvent::PointerMove {
                id: 3,
                x: 300.0,
                y: 100.0,
            },
            &mut view,
        );
        assert_eq!(view, before);
    }
}
