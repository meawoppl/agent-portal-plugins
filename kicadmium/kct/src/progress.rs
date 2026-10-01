//! Progress callback infrastructure (port of `kicad_tools.progress`).
//!
//! A callback receives `(progress, message, cancelable)` -- `progress` in
//! `0.0..=1.0` or `-1.0` for indeterminate -- and returns `false` to cancel.
//! [`ProgressContext`] scopes a callback for the current thread (Python's
//! `ContextVar`); [`report_progress`] reports through the innermost scope.

use std::cell::{Cell, RefCell};
use std::io::Write;
use std::rc::Rc;

use serde_json::{json, Value};

/// Progress callback; returns `false` to cancel.
pub type ProgressCallback = Rc<dyn Fn(f64, &str, bool) -> bool>;

/// Anything that can report progress.
pub trait ProgressReporter {
    fn report(&self, progress: f64, message: &str, cancelable: bool) -> bool;
}

thread_local! {
    static CURRENT: RefCell<Option<ProgressCallback>> = const { RefCell::new(None) };
}

/// Current scoped callback, if any.
pub fn get_current_callback() -> Option<ProgressCallback> {
    CURRENT.with(|c| c.borrow().clone())
}

/// Report through the current scope's callback (`true` when there is none).
pub fn report_progress(progress: f64, message: &str, cancelable: bool) -> bool {
    match get_current_callback() {
        Some(cb) => cb(progress, message, cancelable),
        None => true,
    }
}

/// A progress event for JSON output mode.
#[derive(Debug, Clone, PartialEq)]
pub struct ProgressEvent {
    pub progress: f64,
    pub message: String,
    pub cancelable: bool,
}

impl ProgressEvent {
    pub fn to_dict(&self) -> Value {
        json!({
            "progress": self.progress,
            "message": self.message,
            "cancelable": self.cancelable,
        })
    }

    /// Compact JSON line (Python `json.dumps` spacing).
    pub fn to_json(&self) -> String {
        format!(
            "{{\"progress\": {}, \"message\": {}, \"cancelable\": {}}}",
            crate::utils::pyrepr::py_float_repr(self.progress),
            serde_json::to_string(&self.message).unwrap_or_default(),
            self.cancelable
        )
    }
}

/// Scoped progress reporting: while alive, its callback is the thread's
/// current callback; dropping it restores the previous one. Tracks
/// cancellation (once the callback returns `false`, later reports return
/// `false` without calling it).
pub struct ProgressContext {
    callback: Option<ProgressCallback>,
    previous: Option<Option<ProgressCallback>>,
    cancelled: Cell<bool>,
}

impl ProgressContext {
    /// Enter a scope with `callback` (None: progress is not reported).
    pub fn enter(callback: Option<ProgressCallback>) -> Self {
        let previous = CURRENT.with(|c| c.replace(callback.clone()));
        Self {
            callback,
            previous: Some(previous),
            cancelled: Cell::new(false),
        }
    }

    pub fn report(&self, progress: f64, message: &str, cancelable: bool) -> bool {
        if self.cancelled.get() {
            return false;
        }
        match &self.callback {
            Some(cb) => {
                let result = cb(progress, message, cancelable);
                if !result {
                    self.cancelled.set(true);
                }
                result
            }
            None => true,
        }
    }

    pub fn cancelled(&self) -> bool {
        self.cancelled.get()
    }

    pub fn callback(&self) -> Option<&ProgressCallback> {
        self.callback.as_ref()
    }
}

impl ProgressReporter for ProgressContext {
    fn report(&self, progress: f64, message: &str, cancelable: bool) -> bool {
        ProgressContext::report(self, progress, message, cancelable)
    }
}

impl Drop for ProgressContext {
    fn drop(&mut self) {
        if let Some(prev) = self.previous.take() {
            CURRENT.with(|c| *c.borrow_mut() = prev);
        }
    }
}

/// No-op progress scope (`null_progress()`).
pub fn null_progress() -> ProgressContext {
    ProgressContext::enter(None)
}

/// Callback writing one JSON event per line to `out` (stderr by default);
/// never cancels.
pub fn create_json_callback(out: Option<Rc<RefCell<dyn Write>>>) -> ProgressCallback {
    Rc::new(move |progress, message, cancelable| {
        let event = ProgressEvent {
            progress,
            message: message.to_string(),
            cancelable,
        };
        let line = event.to_json();
        match &out {
            Some(w) => {
                let mut w = w.borrow_mut();
                let _ = writeln!(w, "{line}");
                let _ = w.flush();
            }
            None => eprintln!("{line}"),
        }
        true
    })
}

/// Callback printing `"{pct}%: {message}"` (or just the message when
/// `show_percent` is false or progress is indeterminate).
pub fn create_print_callback(
    out: Option<Rc<RefCell<dyn Write>>>,
    show_percent: bool,
) -> ProgressCallback {
    Rc::new(move |progress, message, _cancelable| {
        let line = if show_percent && progress >= 0.0 {
            format!("{}%: {message}", python_round(progress * 100.0))
        } else {
            message.to_string()
        };
        match &out {
            Some(w) => {
                let mut w = w.borrow_mut();
                let _ = writeln!(w, "{line}");
                let _ = w.flush();
            }
            None => eprintln!("{line}"),
        }
        true
    })
}

/// `f"{x:.0f}"` (round-half-even).
fn python_round(x: f64) -> String {
    format!("{:.0}", x)
}

/// Scales a phase's 0..1 progress into `[start, end]` of a parent callback
/// (indeterminate `-1` passes through), optionally prefixing messages.
#[derive(Clone)]
pub struct SubProgressCallback {
    parent: ProgressCallback,
    start: f64,
    end: f64,
    prefix: String,
}

impl SubProgressCallback {
    pub fn new(parent: ProgressCallback, start: f64, end: f64, prefix: &str) -> Self {
        Self {
            parent,
            start,
            end,
            prefix: prefix.to_string(),
        }
    }

    pub fn call(&self, progress: f64, message: &str, cancelable: bool) -> bool {
        let scaled = if progress < 0.0 {
            -1.0
        } else {
            self.start + progress * (self.end - self.start)
        };
        let full = if self.prefix.is_empty() {
            message.to_string()
        } else {
            format!("{}{message}", self.prefix)
        };
        (self.parent)(scaled, &full, cancelable)
    }

    /// As a plain [`ProgressCallback`].
    pub fn into_callback(self) -> ProgressCallback {
        Rc::new(move |p, m, c| self.call(p, m, c))
    }
}

impl ProgressReporter for SubProgressCallback {
    fn report(&self, progress: f64, message: &str, cancelable: bool) -> bool {
        self.call(progress, message, cancelable)
    }
}
