mod api;
use gloo_net::http::Request;
use serde_json::Value;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;

#[wasm_bindgen(module = "/src/workbench.js")]
extern "C" {
    #[wasm_bindgen(js_name = bootWorkbench)]
    fn boot_workbench(project: &str);
}

const TABS: &[(&str, &str)] = &[
    ("schematic", "Schematic"),
    ("pcb", "PCB"),
    ("gerbers", "Gerbers"),
    ("3d", "3D"),
    ("step", "STEP"),
    ("bom", "BOM"),
    ("libraries", "Libraries"),
    ("analysis", "Analysis"),
    ("panelization", "Panelization"),
    ("checks", "Checks"),
];

#[derive(Properties, PartialEq)]
struct ViewerProps {
    kind: AttrValue,
}
#[function_component(Viewer)]
fn viewer(props: &ViewerProps) -> Html {
    use_effect_with(props.kind.clone(), |_| {
        boot_workbench("");
        || ()
    });
    html! { <div class="viewer-card"><iframe class="native-viewer" data-kind={props.kind.clone()} title={format!("{} viewer",props.kind)} src="/kicad-viewer/runtime.html" /></div> }
}

#[derive(Properties, PartialEq)]
struct RemoteProps {
    endpoint: AttrValue,
    title: AttrValue,
}
#[function_component(RemotePanel)]
fn remote_panel(props: &RemoteProps) -> Html {
    let body = use_state(|| "Loading…".to_owned());
    let endpoint = props.endpoint.clone();
    {
        let body = body.clone();
        use_effect_with(endpoint.clone(), move |endpoint| {
            let endpoint = endpoint.to_string();
            spawn_local(async move {
                let text = match Request::get(&endpoint).send().await {
                    Ok(r) => r.text().await.unwrap_or_else(|e| e.to_string()),
                    Err(e) => e.to_string(),
                };
                body.set(text)
            });
            || ()
        });
    }
    html! {<article class="card"><h2>{&props.title}</h2><pre>{(*body).clone()}</pre></article>}
}

#[function_component(App)]
fn app() -> Html {
    let active = use_state(|| "pcb".to_owned());
    let manifest = use_state(|| None::<Value>);
    {
        let manifest = manifest.clone();
        use_effect_with((), move |_| {
            spawn_local(async move {
                if let Ok(r) = Request::get("/api/kicad/manifest").send().await {
                    if let Ok(v) = r.json::<Value>().await {
                        manifest.set(Some(v));
                    }
                }
            });
            || ()
        });
    }
    {
        use_effect_with((), move |_| {
            boot_workbench("");
            || ()
        });
    }
    let revision = manifest
        .as_ref()
        .and_then(|m| m.get("revision"))
        .and_then(Value::as_str)
        .unwrap_or("loading");
    html! {<div class="app-shell"><header><div><h1>{"kicadmium"}</h1><span class="tagline">{"KiCad's toxic uncle everyone warned you about"}</span></div><div class="status"><span class="live-dot"/>{format!("revision {revision}")}</div></header>
    <nav class="tabs" aria-label="Workbench views">{for TABS.iter().map(|(id,label)|{let owned=(*id).to_owned();let selected=*active==*id;let active=active.clone();html!{<button class={classes!(selected.then_some("active"))} aria-selected={selected.to_string()} onclick={Callback::from(move |_|active.set(owned.clone()))}>{*label}</button>}})}</nav>
    <main>{match active.as_str(){"schematic"=>html!{<Viewer kind="schematic"/>},"pcb"=>html!{<Viewer kind="pcb"/>},"3d"|"step"=>html!{<Viewer kind="model"/>},"bom"=>html!{<RemotePanel title="Bill of materials" endpoint="/api/kicad/bom"/>},"libraries"=>html!{<RemotePanel title="Libraries" endpoint="/api/kicad/library"/>},"checks"=>html!{<div class="grid"><RemotePanel title="Layout quality" endpoint="/api/kicad/quality"/><RemotePanel title="DRC" endpoint="/api/kicad/drc"/><RemotePanel title="ERC" endpoint="/api/kicad/erc"/><RemotePanel title="Heuristic lint" endpoint="/api/kicad/lint"/></div>},"analysis"=>html!{<RemotePanel title="Project manifest" endpoint="/api/kicad/manifest"/>},"gerbers"=>html!{<RemotePanel title="Gerber artifacts" endpoint="/api/kicad/build/status"/>},_=>html!{<RemotePanel title="Panelization" endpoint="/api/kicad/build/status"/>}}}</main></div>}
}
fn main() {
    yew::Renderer::<App>::new().render();
}
