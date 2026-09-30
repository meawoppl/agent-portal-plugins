mod annotate;
mod api;
mod checks;
mod live;
mod viewer;

use annotate::Annotator;
use checks::Checks;
use gloo_net::http::Request;
use live::LiveStatus;
use wasm_bindgen_futures::spawn_local;
use web_sys::HtmlInputElement;
use yew::prelude::*;

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
fn saved_tab() -> String {
    web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|s| s.get_item("kicadmium:tab").ok().flatten())
        .filter(|t| TABS.iter().any(|(id, _)| id == t))
        .unwrap_or_else(|| "pcb".into())
}
fn query_project() -> String {
    web_sys::window()
        .and_then(|w| w.location().search().ok())
        .and_then(|q| {
            q.trim_start_matches('?')
                .split('&')
                .find_map(|p| p.strip_prefix("project=").map(str::to_owned))
        })
        .unwrap_or_default()
}
fn endpoint(path: &str, p: &str) -> String {
    if p.is_empty() {
        path.into()
    } else {
        format!("{path}?project={p}")
    }
}

#[derive(Properties, PartialEq)]
struct RemoteProps {
    title: AttrValue,
    path: AttrValue,
    project: AttrValue,
}
#[function_component(RemotePanel)]
fn remote(props: &RemoteProps) -> Html {
    let text = use_state(|| "Loading…".to_owned());
    {
        let text = text.clone();
        let url = endpoint(&props.path, &props.project);
        use_effect_with(url.clone(), move |url| {
            let url = url.clone();
            spawn_local(async move {
                let body = match Request::get(&url).send().await {
                    Ok(r) => r.text().await.unwrap_or_else(|e| e.to_string()),
                    Err(e) => e.to_string(),
                };
                text.set(body);
            });
            || ()
        });
    }
    html! {<article class="card"><h2>{&props.title}</h2><pre>{(*text).clone()}</pre></article>}
}

#[function_component(App)]
fn app() -> Html {
    let active = use_state(saved_tab);
    let project = use_state(query_project);
    let manifest = use_state(|| None::<shared::ManifestResponse>);
    let workspace = use_state(|| None::<shared::WorkspaceResponse>);
    {
        let workspace = workspace.clone();
        use_effect_with((), move |_| {
            spawn_local(async move { workspace.set(api::workspace().await.ok()) });
            || ()
        });
    }
    {
        let manifest = manifest.clone();
        let p = (*project).clone();
        use_effect_with(p.clone(), move |_| {
            spawn_local(async move {
                manifest.set(api::manifest(&p).await.ok());
            });
            || ()
        });
    }
    let revision = manifest
        .as_ref()
        .map(|m| m.revision.as_str())
        .unwrap_or("loading")
        .to_owned();
    let change_project = {
        let project = project.clone();
        Callback::from(move |e: InputEvent| {
            project.set(e.target_unchecked_into::<HtmlInputElement>().value())
        })
    };
    html! {<div class="app-shell"><header><div><h1>{"kicadmium"}</h1><span class="tagline">{"KiCad's toxic uncle everyone warned you about"}</span></div><div class="header-tools"><input class="project-input" list="projects" aria-label="Project id" placeholder="default project" value={(*project).clone()} oninput={change_project}/><datalist id="projects">{for workspace.as_ref().map(|w|w.projects.iter().map(|p|html!{<option value={p.id.clone()}>{p.name.clone()}</option>}).collect::<Vec<_>>()).unwrap_or_default()}</datalist><LiveStatus project={(*project).clone()}/><Annotator project={(*project).clone()} tab={(*active).clone()} revision={revision.clone()}/></div></header>
    <nav class="tabs" aria-label="Workbench views">{for TABS.iter().map(|(id,label)|{let id=(*id).to_owned();let selected=*active==id;let active=active.clone();html!{<button class={classes!(selected.then_some("active"))} aria-selected={selected.to_string()} onclick={Callback::from(move |_|{if let Some(s)=web_sys::window().and_then(|w|w.local_storage().ok().flatten()){let _=s.set_item("kicadmium:tab",&id);}active.set(id.clone())})}>{*label}</button>}})}</nav>
    <main>{match active.as_str(){"schematic"=>html!{<viewer::Viewer project={(*project).clone()} kind="schematic"/>},"pcb"=>html!{<viewer::Viewer project={(*project).clone()} kind="pcb"/>},"3d"|"step"=>html!{<viewer::Viewer project={(*project).clone()} kind="model"/>},"checks"=>html!{<Checks project={(*project).clone()}/>},"bom"=>html!{<RemotePanel title="Bill of materials" path="/api/kicad/bom" project={(*project).clone()}/>},"libraries"=>html!{<RemotePanel title="Libraries" path="/api/kicad/library" project={(*project).clone()}/>},"gerbers"=>html!{<RemotePanel title="Gerbers" path="/api/build/status" project={(*project).clone()}/>},"analysis"=>html!{<RemotePanel title="Analysis" path="/api/kicad/manifest" project={(*project).clone()}/>},_=>html!{<RemotePanel title="Panelization" path="/api/build/status" project={(*project).clone()}/>}}}</main></div>}
}
fn main() {
    yew::Renderer::<App>::new().render();
}
