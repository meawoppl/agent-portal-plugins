//! Port of upstream tests/test_progress_callback.py.

use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;

use kct::progress::{
    create_json_callback, create_print_callback, get_current_callback, null_progress,
    report_progress, ProgressCallback, ProgressContext, ProgressEvent, SubProgressCallback,
};

type Log<T> = Rc<RefCell<Vec<T>>>;

fn recorder() -> (ProgressCallback, Log<(f64, String)>) {
    let log: Log<(f64, String)> = Rc::default();
    let l = log.clone();
    let cb: ProgressCallback = Rc::new(move |p, m, _| {
        l.borrow_mut().push((p, m.to_string()));
        true
    });
    (cb, log)
}

#[test]
fn context_sets_and_restores_current_callback() {
    let (cb, log) = recorder();
    assert!(get_current_callback().is_none());
    {
        let ctx = ProgressContext::enter(Some(cb));
        assert!(get_current_callback().is_some());
        ctx.report(0.5, "Test message", true);
    }
    assert!(get_current_callback().is_none());
    assert_eq!(*log.borrow(), vec![(0.5, "Test message".to_string())]);
}

#[test]
fn context_tracks_cancellation() {
    let cb: ProgressCallback = Rc::new(|p, _, _| p < 0.5);
    let ctx = ProgressContext::enter(Some(cb));
    assert!(!ctx.cancelled());
    ctx.report(0.25, "Quarter done", true);
    assert!(!ctx.cancelled());
    ctx.report(0.5, "Half done", true);
    assert!(ctx.cancelled());
    assert!(!ctx.report(0.75, "Should not process", true));
}

#[test]
fn null_contexts() {
    let ctx = ProgressContext::enter(None);
    assert!(ctx.report(0.5, "Test", true));
    drop(ctx);
    let ctx = null_progress();
    assert!(ctx.callback().is_none());
    assert!(ctx.report(0.5, "Test", true));
}

#[test]
fn report_progress_uses_current_context() {
    let (cb, log) = recorder();
    assert!(report_progress(0.5, "No context", true));
    assert!(log.borrow().is_empty());
    {
        let _ctx = ProgressContext::enter(Some(cb));
        assert!(report_progress(0.5, "With context", true));
    }
    assert_eq!(*log.borrow(), vec![(0.5, "With context".to_string())]);
}

#[test]
fn nested_contexts_restore_correctly() {
    let (outer, outer_log) = recorder();
    let (inner, inner_log) = recorder();
    {
        let _o = ProgressContext::enter(Some(outer));
        report_progress(0.5, "Outer 1", true);
        {
            let _i = ProgressContext::enter(Some(inner));
            report_progress(0.5, "Inner", true);
        }
        report_progress(0.5, "Outer 2", true);
    }
    let msgs = |l: &Log<(f64, String)>| l.borrow().iter().map(|e| e.1.clone()).collect::<Vec<_>>();
    assert_eq!(msgs(&outer_log), ["Outer 1", "Outer 2"]);
    assert_eq!(msgs(&inner_log), ["Inner"]);
}

#[test]
fn event_dict_and_json() {
    let e = ProgressEvent {
        progress: 0.5,
        message: "Halfway".into(),
        cancelable: true,
    };
    assert_eq!(
        e.to_dict(),
        serde_json::json!({"progress": 0.5, "message": "Halfway", "cancelable": true})
    );
    let e = ProgressEvent {
        progress: 0.75,
        message: "Almost done".into(),
        cancelable: false,
    };
    let j = e.to_json();
    assert_eq!(
        j,
        r#"{"progress": 0.75, "message": "Almost done", "cancelable": false}"#
    );
    let parsed: serde_json::Value = serde_json::from_str(&j).unwrap();
    assert_eq!(parsed["cancelable"], false);
}

#[test]
fn sub_progress_scales_prefixes_and_propagates() {
    let (parent, log) = recorder();
    let p1 = SubProgressCallback::new(parent.clone(), 0.0, 0.5, "");
    for p in [0.0, 0.5, 1.0] {
        p1.call(p, "phase 1", true);
    }
    let p2 = SubProgressCallback::new(parent.clone(), 0.5, 1.0, "");
    for p in [0.0, 0.5, 1.0] {
        p2.call(p, "phase 2", true);
    }
    let got: Vec<f64> = log.borrow().iter().map(|e| e.0).collect();
    for (g, e) in got.iter().zip([0.0, 0.25, 0.5, 0.5, 0.75, 1.0]) {
        assert!((g - e).abs() < 1e-12);
    }
    log.borrow_mut().clear();
    SubProgressCallback::new(parent.clone(), 0.0, 1.0, "Phase 1: ").call(0.5, "Working", true);
    assert_eq!(log.borrow()[0].1, "Phase 1: Working");
    log.borrow_mut().clear();
    SubProgressCallback::new(parent, 0.0, 0.5, "").call(-1.0, "Indeterminate", true);
    assert_eq!(log.borrow()[0].0, -1.0);

    let cancel: ProgressCallback = Rc::new(|_, _, _| false);
    let sub = SubProgressCallback::new(cancel, 0.0, 1.0, "").into_callback();
    assert!(!sub(0.5, "Test", true));
}

struct Buf(Rc<RefCell<Vec<u8>>>);
impl Write for Buf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

type Sink = Rc<RefCell<dyn Write>>;

fn sink() -> (Sink, Rc<RefCell<Vec<u8>>>) {
    let bytes = Rc::new(RefCell::new(Vec::new()));
    let w: Sink = Rc::new(RefCell::new(Buf(bytes.clone())));
    (w, bytes)
}

#[test]
fn json_callback() {
    let (w, bytes) = sink();
    let cb = create_json_callback(Some(w));
    assert!(cb(0.5, "Test message", true));
    assert!(cb(1.0, "End", false));
    let text = String::from_utf8(bytes.borrow().clone()).unwrap();
    let first: serde_json::Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(first["progress"], 0.5);
    assert_eq!(first["message"], "Test message");
    assert_eq!(first["cancelable"], true);
}

#[test]
fn print_callback() {
    let (w, bytes) = sink();
    create_print_callback(Some(w), true)(0.5, "Halfway", true);
    let line = String::from_utf8(bytes.borrow().clone()).unwrap();
    assert_eq!(line.trim(), "50%: Halfway");
    let (w, bytes) = sink();
    create_print_callback(Some(w), false)(0.5, "Halfway", true);
    let line = String::from_utf8(bytes.borrow().clone()).unwrap();
    assert!(!line.contains('%') && line.contains("Halfway"));
}
