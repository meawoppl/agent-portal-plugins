use gloo_net::http::Request;
use serde_json::{json, Value};
use shared::SourcesResponse;
use wasm_bindgen::{closure::Closure, JsCast};
use wasm_bindgen_futures::spawn_local;
use web_sys::{HtmlIFrameElement, MessageEvent};
use yew::prelude::*;

#[derive(Properties, PartialEq)]
pub struct Props {
    pub project: AttrValue,
    pub kind: AttrValue,
    #[prop_or(true)]
    pub active: bool,
}

fn stored(kind: &str) -> Value {
    web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|s| {
            s.get_item(&format!("kicadmium:viewer:{kind}"))
                .ok()
                .flatten()
        })
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!({}))
}

async fn send_snapshot(
    frame: HtmlIFrameElement,
    project: String,
    kind: String,
    active: bool,
    pours: bool,
) {
    let Ok(response) = Request::get(&crate::api::url("/api/kicad/sources", &project))
        .send()
        .await
    else {
        return;
    };
    let Ok(snapshot) = response.json::<SourcesResponse>().await else {
        return;
    };
    let state = stored(&kind);
    let payload = if kind == "model" {
        json!({"type":"kicad-pcb-snapshot","kind":"model","context":kind,"active":active,
            "url":crate::api::url(&format!("/api/kicad/model.glb?rev={}",crate::api::encode(&snapshot.revision)),&project),
            "viewState":state.get("view"),"uiState":state.get("ui")})
    } else {
        json!({"type":"kicad-pcb-snapshot","kind":"native","context":kind,"active":active,
            "revision":snapshot.revision,"sources":snapshot.sources,"polygonPours":pours,
            "viewState":state.get("view"),"uiState":state.get("ui")})
    };
    if let Some(window) = frame.content_window() {
        let _ = window.post_message(
            // Plain JS objects: serde_wasm_bindgen would turn JSON maps into `Map`s.
            &js_sys::JSON::parse(&payload.to_string()).unwrap_or_default(),
            "*",
        );
    }
}

#[function_component(Viewer)]
pub fn viewer(props: &Props) -> Html {
    let node = use_node_ref();
    let pours = use_state(|| true);
    {
        let node = node.clone();
        let project = props.project.to_string();
        let kind = props.kind.to_string();
        let active = props.active;
        let pours = *pours;
        use_effect_with((project.clone(), kind.clone(), active, pours), move |_| {
            let frame = node
                .cast::<HtmlIFrameElement>()
                .expect("viewer iframe mounted");
            spawn_local(send_snapshot(
                frame.clone(),
                project.clone(),
                kind.clone(),
                active,
                pours,
            ));
            let frame_for_event = frame.clone();
            let project_for_event = project.clone();
            let kind_for_event = kind.clone();
            let callback = Closure::<dyn Fn(MessageEvent)>::new(move |event: MessageEvent| {
                let Ok(value) = serde_wasm_bindgen::from_value::<Value>(event.data()) else {
                    return;
                };
                match value.get("type").and_then(Value::as_str) {
                    Some("kicad-pcb-runtime-ready") => spawn_local(send_snapshot(
                        frame_for_event.clone(),
                        project_for_event.clone(),
                        kind_for_event.clone(),
                        active,
                        pours,
                    )),
                    Some("kicad-pcb-view-state")
                        if value.get("context").and_then(Value::as_str)
                            == Some(&kind_for_event) =>
                    {
                        if let Some(storage) =
                            web_sys::window().and_then(|w| w.local_storage().ok().flatten())
                        {
                            let _ = storage.set_item(
                                &format!("kicadmium:viewer:{kind_for_event}"),
                                &value.to_string(),
                            );
                        }
                    }
                    _ => {}
                }
            });
            let window = web_sys::window().unwrap();
            let _ = window
                .add_event_listener_with_callback("message", callback.as_ref().unchecked_ref());
            move || {
                let _ = window.remove_event_listener_with_callback(
                    "message",
                    callback.as_ref().unchecked_ref(),
                );
                drop(callback)
            }
        });
    }
    let toggle = {
        let pours = pours.clone();
        Callback::from(move |_| pours.set(!*pours))
    };
    html! {<div class="viewer-card">{if props.kind.as_str()=="pcb"{html!{<label class="viewer-option"><input type="checkbox" checked={*pours} onchange={toggle}/>{" Polygon pours"}</label>}}else{Html::default()}}<iframe key={props.kind.to_string()} ref={node} class="native-viewer" data-kind={props.kind.clone()} title={format!("{} viewer",props.kind)} src="/kicad-viewer/runtime.html" /></div>}
}
