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

/// Browser-storage key for the PCB "Polygon pours" toggle.
const POURS_KEY: &str = "kicadmium:pcb-pours";

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

/// Remembered pours toggle; shown unless the user turned it off.
fn saved_pours() -> bool {
    storage()
        .and_then(|s| s.get_item(POURS_KEY).ok().flatten())
        .is_none_or(|v| v != "false")
}

#[function_component(Viewer)]
pub fn viewer(props: &Props) -> Html {
    let pours = use_state(saved_pours);
    let is_pcb = props.kind.as_str() == "pcb";
    let is_model = props.kind.as_str() == "model";
    let is_schematic = props.kind.as_str() == "schematic";
    let toggle = {
        let pours = pours.clone();
        Callback::from(move |_| {
            let next = !*pours;
            if let Some(s) = storage() {
                let _ = s.set_item(POURS_KEY, if next { "true" } else { "false" });
            }
            pours.set(next)
        })
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
        html! {<div class="lib-stage-msg">{format!("Unknown viewer kind: {}", props.kind)}</div>}
    };
    html! {<div class={classes!("viewer-card", is_pcb.then_some("rust-pcb"), is_model.then_some("rust-model"), is_schematic.then_some("rust-schematic"))}>{options}{body}</div>}
}
