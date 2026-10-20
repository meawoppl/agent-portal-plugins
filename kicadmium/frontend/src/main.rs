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
fn save_tab(id: &str) {
    if let Some(s) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        let _ = s.set_item("kicadmium:tab", id);
    }
}
/// A URL-decoded, non-empty query parameter of the page URL.
fn query_param(name: &str) -> Option<String> {
    let search = web_sys::window()?.location().search().ok()?;
    web_sys::UrlSearchParams::new_with_str(&search)
        .ok()?
        .get(name)
        .filter(|v| !v.is_empty())
}
/// Deep links (`?tab=`, `?sheet=`, `?ref=`), read once when the page loads.
/// The URL is never rewritten; they only choose what opens first.
#[derive(Clone, Debug, Default, PartialEq)]
struct DeepLink {
    tab: Option<String>,
    sheet: Option<String>,
    reference: Option<String>,
}
impl DeepLink {
    fn from_url() -> Self {
        Self {
            tab: query_param("tab"),
            sheet: query_param("sheet"),
            reference: query_param("ref"),
        }
    }
    /// The tab to open: a known `?tab=`, else Schematic when a sheet or
    /// reference is linked, else the remembered tab.
    fn initial_tab(&self, saved: impl FnOnce() -> String) -> String {
        self.tab
            .clone()
            .filter(|t| TABS.iter().any(|(id, _)| id == t))
            .or_else(|| {
                (self.sheet.is_some() || self.reference.is_some()).then(|| "schematic".into())
            })
            .unwrap_or_else(saved)
    }
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
/// The revision the views load under: the live feed's source hash once it
/// has spoken, else the manifest's source hash, else the manifest's own
/// (per-fetch timestamp) revision. The views key their fetches on it, so it
/// must settle on one value: the old `loading` → timestamp → hash sequence
/// made the 3D view download the board GLB three times.
fn effective_revision(
    live: Option<&str>,
    manifest: Option<&shared::ManifestResponse>,
) -> Option<String> {
    live.map(str::to_owned).or_else(|| {
        let m = manifest?;
        Some(
            m.source_revision
                .clone()
                .unwrap_or_else(|| m.revision.clone()),
        )
    })
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
    use super::{pick_project, DeepLink};

    #[test]
    fn deep_link_tab_wins_then_schematic_for_sheet_or_ref_then_saved() {
        let saved = || "pcb".to_owned();
        let link = |tab: Option<&str>, sheet: Option<&str>, reference: Option<&str>| DeepLink {
            tab: tab.map(Into::into),
            sheet: sheet.map(Into::into),
            reference: reference.map(Into::into),
        };
        assert_eq!(DeepLink::default().initial_tab(saved), "pcb");
        assert_eq!(link(Some("bom"), None, None).initial_tab(saved), "bom");
        assert_eq!(link(Some("nope"), None, None).initial_tab(saved), "pcb");
        assert_eq!(link(None, Some("/a"), None).initial_tab(saved), "schematic");
        assert_eq!(link(None, None, Some("U1")).initial_tab(saved), "schematic");
        assert_eq!(
            link(Some("pcb"), None, Some("U1")).initial_tab(saved),
            "pcb"
        );
    }
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
    fn revision_prefers_live_then_source_hash_then_timestamp() {
        use super::effective_revision;
        let mut m = serde_json::from_value::<shared::ManifestResponse>(serde_json::json!({
            "project": "p", "name": "p", "root": "/w/p", "revision": "1791685733887",
            "files": [], "warnings": [], "kicad_cli": null, "kicad_version": null,
            "source_revision": "dee326246b475da6db37",
        }))
        .unwrap();
        assert_eq!(effective_revision(None, None), None);
        assert_eq!(
            effective_revision(None, Some(&m)).as_deref(),
            Some("dee326246b475da6db37")
        );
        assert_eq!(
            effective_revision(Some("abc"), Some(&m)).as_deref(),
            Some("abc")
        );
        m.source_revision = None;
        assert_eq!(
            effective_revision(None, Some(&m)).as_deref(),
            Some("1791685733887")
        );
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
#[component(App)]
fn app() -> Html {
    // Applied to the first Schematic visit only; any tab or project change
    // drops it so revisiting a tab never jumps back.
    let link = use_state(DeepLink::from_url);
    let active = use_state(|| link.initial_tab(saved_tab));
    // A reference the schematic sent to the PCB tab's search.
    let probe = use_state(String::new);
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
    // Views wait for the manifest (success or failure) so they mount once,
    // under the project's real revision, instead of once per placeholder.
    let manifest_ready = use_state(|| false);
    {
        let manifest = manifest.clone();
        let manifest_ready = manifest_ready.clone();
        let p = (*project).clone();
        use_effect_with(p.clone(), move |_| {
            manifest_ready.set(false);
            let token = request::RequestToken::default();
            let task_token = token.clone();
            spawn_local(async move {
                let next = api::manifest(&p).await.ok();
                if task_token.current() {
                    manifest.set(next);
                    manifest_ready.set(true);
                }
            });
            move || token.cancel()
        });
    }
    // Latest live revision; LiveStatus keeps the callback from its first
    // render, so it sets this state rather than patching `manifest`.
    let live_revision = use_state(|| None::<String>);
    let revision = effective_revision(live_revision.as_deref(), manifest.as_ref())
        .unwrap_or_else(|| "loading".to_owned());
    let change_project = {
        let project = project.clone();
        let live_revision = live_revision.clone();
        let workspace = workspace.clone();
        let link = link.clone();
        Callback::from(move |e: InputEvent| {
            live_revision.set(None);
            link.set(DeepLink::default());
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
        let link = link.clone();
        Callback::from(move |e: Event| {
            live_revision.set(None);
            link.set(DeepLink::default());
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
    let has_pcb = manifest
        .as_ref()
        .is_some_and(|m| m.files.iter().any(|f| f.kind == "pcb"));
    // "Show on PCB": open the PCB tab searching for the reference.
    let on_cross_probe = has_pcb.then(|| {
        let (active, probe, link) = (active.clone(), probe.clone(), link.clone());
        Callback::from(move |reference: String| {
            save_tab("pcb");
            link.set(DeepLink::default());
            probe.set(reference);
            active.set("pcb".into())
        })
    });
    let (sheet, reveal) = if *active == "schematic" {
        (
            link.sheet.clone().map(AttrValue::from),
            link.reference.clone().map(AttrValue::from),
        )
    } else {
        (None, None)
    };
    let on_revision = {
        let live_revision = live_revision.clone();
        Callback::from(move |revision: String| live_revision.set(Some(revision)))
    };
    html! {<div class="app-shell"><header><div><h1>{"kicadmium"}</h1><span class="tagline">{"The heavy metal your PCBs were missing."}</span></div><div class="header-tools">{picker}<LiveStatus project={(*project).clone()} {on_revision}/></div></header>
    <build_strip::BuildStrip project={(*project).clone()}/>
    <nav class="tabs" aria-label="Workbench views">{for TABS.iter().map(|(id,label)|{let id=(*id).to_owned();let selected=*active==id;let active=active.clone();let link=link.clone();let probe=probe.clone();html!{<button class={classes!(selected.then_some("active"))} aria-selected={selected.to_string()} onclick={Callback::from(move |_|{save_tab(&id);link.set(DeepLink::default());probe.set(String::new());active.set(id.clone())})}>{*label}</button>}})}</nav>
    <main>{{let view=if !*resolved || !*manifest_ready{html!{<p class="view-status" role="status">{"Opening project…"}</p>}}else{match active.as_str(){"schematic"=>html!{<viewer::Viewer project={(*project).clone()} kind="schematic" revision={revision.clone()} {sheet} {reveal} {on_cross_probe}/>},"pcb"=>html!{<viewer::Viewer project={(*project).clone()} kind="pcb" revision={revision.clone()} query={(*probe).clone()}/>} ,"3d"=>html!{<viewer::Viewer project={(*project).clone()} kind="model" revision={revision.clone()} model={manifest.as_ref().map(|m| m.model.clone()).unwrap_or_default()}/>} ,"checks"=>html!{<Checks project={(*project).clone()}/>} ,"bom"=>html!{<bom::BomTab project={(*project).clone()} revision={revision.clone()}/>} ,"libraries"=>html!{<library::LibraryTab project={(*project).clone()}/>} ,"gerbers"=>html!{<gerbers::GerberTab project={(*project).clone()} revision={revision.clone()}/>} ,_=>html!{<misc::AnalysisTab project={(*project).clone()} revision={revision.clone()}/>}}};
    // Annotation targets what is on screen, so it floats over the visual views only.
    if ANNOTATABLE.contains(&active.as_str()){html!{<div class={classes!("view-stage",format!("view-{}",*active))}>{view}<div class="view-annotate"><Annotator project={(*project).clone()} tab={(*active).clone()} revision={revision.clone()}/></div></div>}}else{view}}}</main></div>}
}
fn main() {
    yew::Renderer::<App>::new().render();
}
