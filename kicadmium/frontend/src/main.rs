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
mod request;
mod schematic_view;
mod vector_scene;
mod view_input;
mod viewer;

use annotate::Annotator;
use checks::Checks;
use live::LiveStatus;
use wasm_bindgen_futures::spawn_local;
use web_sys::HtmlInputElement;
use yew::prelude::*;

/// Tabs whose content can be annotated: the drawn design views.
const ANNOTATABLE: &[&str] = &["schematic", "pcb", "gerbers", "3d"];

const TABS: &[(&str, &str)] = &[
    ("schematic", "Schematic"),
    ("pcb", "PCB"),
    ("gerbers", "Gerbers"),
    ("3d", "3D"),
    ("bom", "BOM"),
    ("libraries", "Libraries"),
    ("analysis", "Analysis"),
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
const PROJECT_KEY: &str = "kicadmium:project";
fn saved_project() -> Option<String> {
    web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|s| s.get_item(PROJECT_KEY).ok().flatten())
}
fn save_project(id: &str) {
    if let Some(s) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        let _ = s.set_item(PROJECT_KEY, id);
    }
}
/// The project to open: an explicit `?project=`, else the last one picked
/// that still exists, else the configured `defaultProject`, else the first
/// project. Empty only for a workspace without projects.
fn pick_project(query: &str, saved: Option<&str>, w: &shared::WorkspaceResponse) -> String {
    let known = |id: &str| w.projects.iter().any(|p| p.id == id);
    if !query.is_empty() {
        return query.to_owned();
    }
    saved
        .filter(|id| known(id))
        .or_else(|| Some(w.default_project.as_str()).filter(|id| known(id)))
        .or_else(|| w.projects.first().map(|p| p.id.as_str()))
        .unwrap_or_default()
        .to_owned()
}
#[cfg(test)]
mod tests {
    use super::pick_project;
    use shared::{ProjectSummary, WorkspaceResponse};

    fn workspace(default: &str, ids: &[&str]) -> WorkspaceResponse {
        WorkspaceResponse {
            cwd: "/w".into(),
            session: String::new(),
            default_project: default.into(),
            projects: ids
                .iter()
                .map(|id| ProjectSummary {
                    id: (*id).into(),
                    name: (*id).into(),
                    root: (*id).into(),
                })
                .collect(),
        }
    }

    #[test]
    fn project_resolution_order() {
        let w = workspace("tesla", &["module", "tesla", "carrier"]);
        assert_eq!(pick_project("carrier", Some("module"), &w), "carrier");
        assert_eq!(pick_project("", Some("module"), &w), "module");
        // A remembered project from another workspace is ignored.
        assert_eq!(pick_project("", Some("gone"), &w), "tesla");
        assert_eq!(pick_project("", None, &w), "tesla");
        let unset = workspace("", &["module", "tesla"]);
        assert_eq!(pick_project("", None, &unset), "module");
        assert_eq!(pick_project("", None, &workspace("", &[])), "");
    }
}
#[function_component(App)]
fn app() -> Html {
    let active = use_state(saved_tab);
    let project = use_state(query_project);
    let manifest = use_state(|| None::<shared::ManifestResponse>);
    let workspace = use_state(|| None::<shared::WorkspaceResponse>);
    // Tabs wait for the project to resolve so the board loads once, under
    // its real id, and the picker never shows a blank placeholder.
    let resolved = use_state(|| !query_project().is_empty());
    {
        let workspace = workspace.clone();
        let project = project.clone();
        let resolved = resolved.clone();
        use_effect_with((), move |_| {
            spawn_local(async move {
                let fetched = api::workspace().await.ok();
                if let Some(w) = &fetched {
                    project.set(pick_project(
                        &query_project(),
                        saved_project().as_deref(),
                        w,
                    ));
                }
                workspace.set(fetched);
                resolved.set(true);
            });
            || ()
        });
    }
    {
        let manifest = manifest.clone();
        let p = (*project).clone();
        use_effect_with(p.clone(), move |_| {
            let token = request::RequestToken::default();
            let task_token = token.clone();
            spawn_local(async move {
                let next = api::manifest(&p).await.ok();
                if task_token.current() {
                    manifest.set(next);
                }
            });
            move || token.cancel()
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
        let live_revision = live_revision.clone();
        let workspace = workspace.clone();
        Callback::from(move |e: InputEvent| {
            live_revision.set(None);
            let id = e.target_unchecked_into::<HtmlInputElement>().value();
            // Remember complete ids only, not every keystroke.
            if workspace
                .as_ref()
                .is_some_and(|w| w.projects.iter().any(|p| p.id == id))
            {
                save_project(&id);
            }
            project.set(id)
        })
    };
    // A real dropdown: an <input list> datalist filters its suggestions by the
    // current value, so a pre-selected project hid every other board.
    let pick_project = {
        let project = project.clone();
        let live_revision = live_revision.clone();
        Callback::from(move |e: Event| {
            live_revision.set(None);
            let id = e
                .target_unchecked_into::<web_sys::HtmlSelectElement>()
                .value();
            save_project(&id);
            project.set(id)
        })
    };
    let picker = match workspace.as_ref().filter(|w| !w.projects.is_empty()) {
        Some(w) => {
            let current = (*project).clone();
            let unknown = (!current.is_empty() && !w.projects.iter().any(|p| p.id == current))
                .then(|| html! {<option value={current.clone()} selected=true>{current.clone()}</option>});
            html! {<select class="project-input" aria-label="Project" onchange={pick_project}>
                {for w.projects.iter().map(|p| {
                    let label = if p.name.is_empty() || p.name == p.id { p.id.clone() } else { format!("{} ({})", p.name, p.id) };
                    html!{<option value={p.id.clone()} selected={p.id == current}>{label}</option>}
                })}
                {unknown.unwrap_or_default()}
            </select>}
        }
        None => {
            html! {<input class="project-input" aria-label="Project id" placeholder="default project" value={(*project).clone()} oninput={change_project}/>}
        }
    };
    let on_revision = {
        let live_revision = live_revision.clone();
        Callback::from(move |revision: String| live_revision.set(Some(revision)))
    };
    html! {<div class="app-shell"><header><div><h1>{"kicadmium"}</h1><span class="tagline">{"The heavy metal your PCBs were missing."}</span></div><div class="header-tools">{picker}<LiveStatus project={(*project).clone()} {on_revision}/></div></header>
    <build_strip::BuildStrip project={(*project).clone()}/>
    <nav class="tabs" aria-label="Workbench views">{for TABS.iter().map(|(id,label)|{let id=(*id).to_owned();let selected=*active==id;let active=active.clone();html!{<button class={classes!(selected.then_some("active"))} aria-selected={selected.to_string()} onclick={Callback::from(move |_|{if let Some(s)=web_sys::window().and_then(|w|w.local_storage().ok().flatten()){let _=s.set_item("kicadmium:tab",&id);}active.set(id.clone())})}>{*label}</button>}})}</nav>
    <main>{{let view=if !*resolved{html!{<p class="view-status" role="status">{"Opening project…"}</p>}}else{match active.as_str(){"schematic"=>html!{<viewer::Viewer project={(*project).clone()} kind="schematic" revision={revision.clone()}/>},"pcb"=>html!{<viewer::Viewer project={(*project).clone()} kind="pcb" revision={revision.clone()}/>} ,"3d"=>html!{<viewer::Viewer project={(*project).clone()} kind="model" revision={revision.clone()}/>} ,"checks"=>html!{<Checks project={(*project).clone()}/>} ,"bom"=>html!{<bom::BomTab project={(*project).clone()} revision={revision.clone()}/>} ,"libraries"=>html!{<library::LibraryTab project={(*project).clone()}/>} ,"gerbers"=>html!{<gerbers::GerberTab project={(*project).clone()} revision={revision.clone()}/>} ,_=>html!{<misc::AnalysisTab project={(*project).clone()} revision={revision.clone()}/>}}};
    // Annotation targets what is on screen, so it floats over the visual views only.
    if ANNOTATABLE.contains(&active.as_str()){html!{<div class={classes!("view-stage",format!("view-{}",*active))}>{view}<div class="view-annotate"><Annotator project={(*project).clone()} tab={(*active).clone()} revision={revision.clone()}/></div></div>}}else{view}}}</main></div>}
}
fn main() {
    yew::Renderer::<App>::new().render();
}
