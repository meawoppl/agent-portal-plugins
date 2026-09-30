//! `<iframe src="/kicad-viewer/runtime.html">` wrapper speaking the viewer
//! runtime's postMessage protocol: the runtime announces
//! `kicad-pcb-runtime-ready`, then receives a `kicad-pcb-snapshot` message.
//! The current `snapshot` prop is (re)posted on every ready announcement and
//! whenever it changes; other messages from this frame go to `on_message`.

use std::cell::RefCell;
use std::rc::Rc;

use gloo_events::EventListener;
use serde_json::Value;
use wasm_bindgen::JsCast;
use web_sys::{HtmlIFrameElement, MessageEvent};
use yew::prelude::*;

pub const RUNTIME_URL: &str = "/kicad-viewer/runtime.html";

#[derive(Properties, PartialEq)]
pub struct RuntimeFrameProps {
    /// Snapshot message to deliver; `None` waits.
    pub snapshot: Option<Rc<Value>>,
    #[prop_or_default]
    pub class: Classes,
    #[prop_or_default]
    pub title: AttrValue,
    /// Non-ready messages the runtime posts back (e.g. view state).
    #[prop_or_default]
    pub on_message: Option<Callback<Value>>,
}

fn post(frame: &NodeRef, snapshot: &Option<Rc<Value>>) {
    let (Some(frame), Some(snapshot)) = (frame.cast::<HtmlIFrameElement>(), snapshot) else {
        return;
    };
    let Some(target) = frame.content_window() else {
        return;
    };
    let origin = web_sys::window()
        .and_then(|w| w.location().origin().ok())
        .unwrap_or_else(|| "*".into());
    if let Ok(message) = js_sys::JSON::parse(&snapshot.to_string()) {
        let _ = target.post_message(&message, &origin);
    }
}

#[function_component(RuntimeFrame)]
pub fn runtime_frame(props: &RuntimeFrameProps) -> Html {
    let frame = use_node_ref();
    let latest = use_mut_ref(|| None::<Rc<Value>>);
    let on_message = use_mut_ref(|| None::<Callback<Value>>);
    *on_message.borrow_mut() = props.on_message.clone();

    {
        let frame = frame.clone();
        let latest = latest.clone();
        use_effect_with(props.snapshot.clone(), move |snapshot| {
            *latest.borrow_mut() = snapshot.clone();
            post(&frame, snapshot);
        });
    }
    {
        let frame = frame.clone();
        let latest: Rc<RefCell<Option<Rc<Value>>>> = latest.clone();
        use_effect_with((), move |_| {
            let window = web_sys::window().expect("window");
            let listener = EventListener::new(&window, "message", move |event| {
                let Some(event) = event.dyn_ref::<MessageEvent>() else {
                    return;
                };
                let Some(own) = frame
                    .cast::<HtmlIFrameElement>()
                    .and_then(|f| f.content_window())
                else {
                    return;
                };
                let from_own = event
                    .source()
                    .and_then(|s| s.dyn_into::<web_sys::Window>().ok())
                    .is_some_and(|s| s == own);
                if !from_own {
                    return;
                }
                let data = js_sys::JSON::stringify(&event.data())
                    .ok()
                    .and_then(|s| s.as_string())
                    .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                    .unwrap_or(Value::Null);
                if data.get("type").and_then(Value::as_str) == Some("kicad-pcb-runtime-ready") {
                    post(&frame, &latest.borrow());
                } else if let Some(callback) = on_message.borrow().as_ref() {
                    callback.emit(data);
                }
            });
            move || drop(listener)
        });
    }

    html! {
        <iframe ref={frame} class={props.class.clone()} title={props.title.clone()} src={RUNTIME_URL} />
    }
}
