use gloo_net::http::Request;
use serde_json::{json, Value};
use shared::SourcesResponse;
use wasm_bindgen::{closure::Closure, JsCast};
use wasm_bindgen_futures::spawn_local;
use web_sys::{HtmlIFrameElement, MessageEvent};
use yew::prelude::*;

use crate::{model_view::ModelView, pcb_view::PcbView, schematic_view::SchematicView};

#[derive(Properties, PartialEq)]
pub struct Props {
    pub project: AttrValue,
    pub kind: AttrValue,
    #[prop_or(true)]
    pub active: bool,
    /// Workbench revision (drives the Rust PCB view's refresh).
    #[prop_or_default]
    pub revision: AttrValue,
}

fn local_storage() -> Option<web_sys::Storage> {
    web_sys::window().and_then(|w| w.local_storage().ok().flatten())
}

fn stored(kind: &str) -> Value {
    local_storage()
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
    let is_pcb = props.kind.as_str() == "pcb";
    let is_model = props.kind.as_str() == "model";
    let is_schematic = props.kind.as_str() == "schematic";
    {
        let node = node.clone();
        let project = props.project.to_string();
        let kind = props.kind.to_string();
        let active = props.active;
        let pours = *pours;
        use_effect_with(
            (project.clone(), kind.clone(), active, pours, is_pcb),
            move |_| {
                // The Rust PCB view renders no iframe.
                let frame = node.cast::<HtmlIFrameElement>();
                let listener = frame.map(|frame| {
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
                    let callback =
                        Closure::<dyn Fn(MessageEvent)>::new(move |event: MessageEvent| {
                            let Ok(value) = serde_wasm_bindgen::from_value::<Value>(event.data())
                            else {
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
                                    if let Some(storage) = local_storage() {
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
                    let _ = window.add_event_listener_with_callback(
                        "message",
                        callback.as_ref().unchecked_ref(),
                    );
                    (window, callback)
                });
                move || {
                    if let Some((window, callback)) = listener {
                        let _ = window.remove_event_listener_with_callback(
                            "message",
                            callback.as_ref().unchecked_ref(),
                        );
                        drop(callback)
                    }
                }
            },
        );
    }
    let toggle = {
        let pours = pours.clone();
        Callback::from(move |_| pours.set(!*pours))
    };
    let options = if is_pcb {
        html! {<div class="viewer-options">
            <label class="viewer-option"><input type="checkbox" checked={*pours} onchange={toggle}/>{" Polygon pours"}</label>
        </div>}
    } else {
        Html::default()
    };
    let body = if is_pcb {
        html! {<PcbView project={props.project.clone()} revision={props.revision.clone()} pours={*pours}/>}
    } else if is_model {
        html! {<ModelView project={props.project.clone()} revision={props.revision.clone()} active={props.active}/>}
    } else if is_schematic {
        html! {<SchematicView project={props.project.clone()} revision={props.revision.clone()}/>}
    } else {
        html! {<iframe key={props.kind.to_string()} ref={node} class="native-viewer" data-kind={props.kind.clone()} title={format!("{} viewer",props.kind)} src="/kicad-viewer/runtime.html" />}
    };
    html! {<div class={classes!("viewer-card", is_pcb.then_some("rust-pcb"), is_model.then_some("rust-model"), is_schematic.then_some("rust-schematic"))}>{options}{body}</div>}
}
