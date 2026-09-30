//! Gerbers tab: pastebom's `gerber-view` crate rendering the current build's
//! Gerber/drill files (or the published ones while a build is pending), with
//! a layer/file side panel.

use std::cell::RefCell;
use std::rc::Rc;

use gerber_view::GerberViewer;
use serde_json::Value;
use shared::GerberSourcesResponse;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::{spawn_local, JsFuture};
use web_sys::HtmlElement;
use yew::prelude::*;

use crate::api;

#[derive(Properties, PartialEq)]
pub struct GerberTabProps {
    pub project: AttrValue,
    /// Current source revision; a change re-fetches sources.
    #[prop_or_default]
    pub revision: AttrValue,
}

#[derive(Default)]
struct Viewer {
    viewer: Option<GerberViewer>,
    on_change: Option<Closure<dyn FnMut(JsValue)>>,
    source_key: String,
}

fn to_json(value: &JsValue) -> Value {
    js_sys::JSON::stringify(value)
        .ok()
        .and_then(|s| s.as_string())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null)
}

fn source_key(sources: &GerberSourcesResponse) -> String {
    format!(
        "{}-{}-{}",
        sources.origin,
        sources.stage.as_deref().unwrap_or(""),
        sources.revision.as_deref().unwrap_or("")
    )
}

/// Fetch every file, refusing HTML error pages masquerading as Gerbers.
async fn payload(sources: &GerberSourcesResponse) -> Result<js_sys::Array, String> {
    let revision = sources.revision.as_deref().unwrap_or("current");
    let array = js_sys::Array::new();
    for item in &sources.files {
        let url = format!("{}&rev={}", item.url, api::encode(revision));
        let bytes = api::get_bytes(&url).await?;
        let head = String::from_utf8_lossy(&bytes[..bytes.len().min(80)])
            .trim_start()
            .to_lowercase();
        if head.starts_with("<!doctype") || head.starts_with("<html") {
            return Err(format!("{} returned HTML from {url}", item.path));
        }
        let entry = js_sys::Object::new();
        let name = item.path.rsplit('/').next().unwrap_or(&item.path);
        js_sys::Reflect::set(&entry, &"name".into(), &name.into()).ok();
        js_sys::Reflect::set(
            &entry,
            &"content".into(),
            &js_sys::Uint8Array::from(bytes.as_slice()).into(),
        )
        .ok();
        array.push(&entry);
    }
    Ok(array)
}

#[function_component(GerberTab)]
pub fn gerber_tab(props: &GerberTabProps) -> Html {
    let host = use_node_ref();
    let state = use_mut_ref(Viewer::default);
    let sources = use_state(|| None::<GerberSourcesResponse>);
    let summary = use_state(|| Value::Null);
    let status = use_state(|| "Loading Gerber viewer...".to_string());

    {
        let host = host.clone();
        let state = state.clone();
        let sources = sources.clone();
        let summary = summary.clone();
        let status = status.clone();
        use_effect_with(
            (props.project.clone(), props.revision.clone()),
            move |(project, _)| {
                let project = project.to_string();
                spawn_local(async move {
                    if let Err(err) = load(&project, &host, &state, &sources, &summary).await {
                        status.set(format!("Gerber viewer failed: {err}"));
                        return;
                    }
                    let count = summary
                        .get("layers")
                        .and_then(Value::as_array)
                        .map_or(0, Vec::len);
                    status.set(match (&*sources, count) {
                        (Some(s), _) if s.files.is_empty() => {
                            "No generated Gerber or drill files found.".into()
                        }
                        (_, 1) => "1 layer loaded".into(),
                        (_, n) => format!("{n} layers loaded"),
                    });
                });
            },
        );
    }
    {
        let state = state.clone();
        use_effect_with((), move |_| {
            move || {
                let mut state = state.borrow_mut();
                if let Some(viewer) = state.viewer.take() {
                    viewer.destroy();
                }
                state.on_change = None;
            }
        });
    }

    let list = |key: &str| -> Vec<Value> {
        summary
            .get(key)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    let layers = list("layers").into_iter().map(|layer| {
        let label = layer.get("label").and_then(Value::as_str).unwrap_or("");
        let name = layer.get("name").and_then(Value::as_str).unwrap_or("");
        html! { <li>{format!("{label} · {name}")}</li> }
    });
    let warnings = list("warnings").into_iter().map(|w| {
        html! { <div class="warning">{w.as_str().unwrap_or_default().to_string()}</div> }
    });
    let skipped = list("skipped");
    let files = sources
        .as_ref()
        .map(|s| s.files.clone())
        .unwrap_or_default();

    html! {
        <div class="card">
            <h2>{"Gerbers"}</h2>
            <div class="gerber-layout">
                <div class="gerber-viewer" ref={host} />
                <aside class="side-panel">
                    <h3>{"Layers"}</h3>
                    <div class="muted">{(*status).clone()}</div>
                    <ul>{for layers}</ul>
                    <h3>{"Files"}</h3>
                    <div class="muted">{sources.as_ref().map(|s| s.label.clone()).unwrap_or_default()}</div>
                    <ul>
                        if files.is_empty() { <li class="muted">{"No Gerber or drill files yet."}</li> }
                        {for files.iter().map(|f| html! { <li><a href={format!("{}&download=1", f.url)}>{&f.path}</a></li> })}
                    </ul>
                    {for warnings}
                    if !skipped.is_empty() {
                        <div class="muted">
                            <h3>{"Skipped"}</h3>
                            {for skipped.iter().map(|s| html! { <div>{s.as_str().unwrap_or_default().to_string()}</div> })}
                        </div>
                    }
                </aside>
            </div>
        </div>
    }
}

async fn load(
    project: &str,
    host: &NodeRef,
    state: &Rc<RefCell<Viewer>>,
    sources: &UseStateHandle<Option<GerberSourcesResponse>>,
    summary: &UseStateHandle<Value>,
) -> Result<(), String> {
    let next = api::gerbers(project).await?;
    let key = source_key(&next);
    sources.set(Some(next.clone()));
    if next.files.is_empty() || state.borrow().source_key == key {
        return Ok(());
    }
    let files = payload(&next).await?;
    if state.borrow().viewer.is_none() {
        let element = host
            .cast::<HtmlElement>()
            .ok_or("Gerber viewer host is not mounted")?;
        let options =
            js_sys::JSON::parse(r##"{"controls":true,"background":"#11131d","padding":18}"##)
                .map_err(|err| format!("{err:?}"))?;
        let viewer = GerberViewer::new(options).map_err(|err| format!("{err:?}"))?;
        viewer.mount(element).map_err(|err| format!("{err:?}"))?;
        let summary = summary.clone();
        let on_change = Closure::<dyn FnMut(JsValue)>::new(move |project: JsValue| {
            summary.set(to_json(&project));
        });
        viewer.on_change(Some(
            on_change
                .as_ref()
                .unchecked_ref::<js_sys::Function>()
                .clone(),
        ));
        let mut state = state.borrow_mut();
        state.viewer = Some(viewer);
        state.on_change = Some(on_change);
    }
    let promise = state
        .borrow()
        .viewer
        .as_ref()
        .map(|viewer| viewer.set_sources(files.into()))
        .ok_or("Gerber viewer unavailable")?;
    let project = JsFuture::from(promise)
        .await
        .map_err(|err| format!("{err:?}"))?;
    summary.set(to_json(&project));
    let mut state = state.borrow_mut();
    if let Some(viewer) = &state.viewer {
        viewer.fit();
        viewer.resize();
    }
    state.source_key = key;
    Ok(())
}
