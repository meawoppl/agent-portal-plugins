mod annotate;
mod api;
mod bom;
mod build_strip;
mod checks;
mod gerbers;
mod library;
mod live;
mod misc;
mod model_view;
mod pcb_view;
mod runtime_frame;
mod viewer;

use annotate::Annotator;
use checks::Checks;
use live::LiveStatus;
use wasm_bindgen_futures::spawn_local;
use web_sys::HtmlInputElement;
use yew::prelude::*;

const TABS: &[(&str, &str)] = &[
    ("schematic", "Schematic"),
    ("pcb", "PCB"),
    ("gerbers", "Gerbers"),
    ("3d", "3D"),
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
    // Latest live revision; LiveStatus keeps the callback from its first
    // render, so it sets this state rather than patching `manifest`.
    let live_revision = use_state(|| None::<String>);
    let revision = live_revision
        .as_ref()
        .cloned()
        .or_else(|| manifest.as_ref().map(|m| m.revision.clone()))
        .unwrap_or_else(|| "loading".to_owned());
    let change_project = {
        let project = project.clone();
        Callback::from(move |e: InputEvent| {
            project.set(e.target_unchecked_into::<HtmlInputElement>().value())
        })
    };
    let on_revision = {
        let live_revision = live_revision.clone();
        Callback::from(move |revision: String| live_revision.set(Some(revision)))
    };
    html! {<div class="app-shell"><header><div><h1>{"kicadmium"}</h1><span class="tagline">{"The heavy metal your PCBs were missing."}</span></div><div class="header-tools"><input class="project-input" list="projects" aria-label="Project id" placeholder="default project" value={(*project).clone()} oninput={change_project}/><datalist id="projects">{for workspace.as_ref().map(|w|w.projects.iter().map(|p|html!{<option value={p.id.clone()}>{p.name.clone()}</option>}).collect::<Vec<_>>()).unwrap_or_default()}</datalist><LiveStatus project={(*project).clone()} {on_revision}/><Annotator project={(*project).clone()} tab={(*active).clone()} revision={revision.clone()}/></div></header>
    <build_strip::BuildStrip project={(*project).clone()}/>
    <nav class="tabs" aria-label="Workbench views">{for TABS.iter().map(|(id,label)|{let id=(*id).to_owned();let selected=*active==id;let active=active.clone();html!{<button class={classes!(selected.then_some("active"))} aria-selected={selected.to_string()} onclick={Callback::from(move |_|{if let Some(s)=web_sys::window().and_then(|w|w.local_storage().ok().flatten()){let _=s.set_item("kicadmium:tab",&id);}active.set(id.clone())})}>{*label}</button>}})}</nav>
    <main>{match active.as_str(){"schematic"=>html!{<viewer::Viewer project={(*project).clone()} kind="schematic"/>},"pcb"=>html!{<viewer::Viewer project={(*project).clone()} kind="pcb" revision={revision.clone()}/>},"3d"=>html!{<viewer::Viewer project={(*project).clone()} kind="model" revision={revision.clone()}/>},"checks"=>html!{<Checks project={(*project).clone()}/>},"bom"=>html!{<bom::BomTab project={(*project).clone()} revision={revision.clone()}/>},"libraries"=>html!{<library::LibraryTab project={(*project).clone()}/>},"gerbers"=>html!{<gerbers::GerberTab project={(*project).clone()} revision={revision.clone()}/>},"analysis"=>html!{<misc::AnalysisTab project={(*project).clone()} revision={revision.clone()}/>},_=>html!{<misc::PanelizationTab project={(*project).clone()} revision={revision.clone()}/>}}}</main></div>}
}
fn main() {
    yew::Renderer::<App>::new().render();
}
