//! Analysis tab: manifest-driven inventory view.

use shared::ManifestResponse;
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;

use crate::api;

#[derive(Properties, PartialEq)]
pub struct TabProps {
    pub project: AttrValue,
    #[prop_or_default]
    pub revision: AttrValue,
}

#[hook]
fn use_manifest(
    project: AttrValue,
    revision: AttrValue,
) -> Option<Result<ManifestResponse, String>> {
    let manifest = use_state(|| None);
    {
        let manifest = manifest.clone();
        use_effect_with((project, revision), move |(project, _)| {
            manifest.set(None);
            let project = project.to_string();
            let token = crate::request::RequestToken::default();
            let task_token = token.clone();
            spawn_local(async move {
                let next = api::manifest(&project).await;
                if task_token.current() {
                    manifest.set(Some(next));
                }
            });
            move || token.cancel()
        });
    }
    (*manifest).clone()
}

fn loading_or_error(state: &Option<Result<ManifestResponse, String>>) -> Option<Html> {
    match state {
        None => Some(html! { <p class="muted">{"Loading project manifest..."}</p> }),
        Some(Err(err)) => {
            Some(html! { <p class="warning">{format!("Unable to load manifest: {err}")}</p> })
        }
        Some(Ok(_)) => None,
    }
}

#[function_component(AnalysisTab)]
pub fn analysis_tab(props: &TabProps) -> Html {
    let manifest = use_manifest(props.project.clone(), props.revision.clone());
    let body = loading_or_error(&manifest).unwrap_or_else(|| {
        let m = manifest.and_then(Result::ok).expect("manifest loaded");
        let mut kinds = std::collections::BTreeMap::<&str, usize>::new();
        for file in &m.files {
            *kinds.entry(file.kind.as_str()).or_default() += 1;
        }
        html! {
            <>
                <p>
                    {format!("Root {} · revision {} · ", m.root, m.revision)}
                    {match (&m.kicad_cli, &m.kicad_version) {
                        (Some(cli), Some(version)) => format!("KiCad {version} ({cli})"),
                        (Some(cli), None) => format!("kicad-cli {cli}"),
                        _ => "kicad-cli not found: checks and exports are unavailable".into(),
                    }}
                </p>
                {for m.warnings.iter().map(|w| html! { <div class="warning">{w}</div> })}
                <table class="bom-table">
                    <thead><tr><th>{"Kind"}</th><th>{"Files"}</th></tr></thead>
                    <tbody>{for kinds.iter().map(|(kind, n)| html! { <tr><td>{*kind}</td><td>{*n}</td></tr> })}</tbody>
                </table>
                <p class="muted">
                    {"Layout-quality audits, DRC/ERC, and kct lint heuristics live on the Checks tab; "}
                    {"deeper analysis runs through "}<code>{"kicadmium kct -- <command>"}</code>{" (rjwalters/kicad-tools)."}
                </p>
            </>
        }
    });
    html! { <div class="card"><h2>{"Analysis"}</h2>{body}</div> }
}
