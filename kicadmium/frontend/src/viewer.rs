use yew::prelude::*;

use crate::{
    model_view::{ModelView, ViewOptions, FINISHES},
    pcb_view::PcbView,
    schematic_view::SchematicView,
};

#[derive(Properties, PartialEq)]
pub struct Props {
    pub project: AttrValue,
    pub kind: AttrValue,
    #[prop_or(true)]
    pub active: bool,
    /// Workbench revision (drives the Rust PCB view's refresh).
    #[prop_or_default]
    pub revision: AttrValue,
    /// The project's 3D settings (default finish).
    #[prop_or_default]
    pub model: shared::ModelConfig,
}

const FINISH_KEY: &str = "kicadmium:3d-finish";
const XRAY_KEY: &str = "kicadmium:3d-xray";
const INNER_KEY: &str = "kicadmium:3d-inner";
const OUTLINE_KEY: &str = "kicadmium:3d-outline";

fn saved_flag(key: &str, default: bool) -> bool {
    storage()
        .and_then(|s| s.get_item(key).ok().flatten())
        .map_or(default, |v| v == "true")
}

fn save_flag(key: &str, value: bool) {
    if let Some(s) = storage() {
        let _ = s.set_item(key, if value { "true" } else { "false" });
    }
}

/// The finish to start from: what the person last picked here, else the
/// project's configured default, else the board's own export.
pub fn initial_finish(saved: Option<&str>, configured: Option<&str>) -> String {
    let known = |v: &str| FINISHES.iter().any(|(value, _)| *value == v);
    saved
        .filter(|v| known(v))
        .or_else(|| configured.filter(|v| known(v)))
        .unwrap_or("board")
        .to_string()
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
    let configured_finish = props.model.finish.clone();
    let finish = use_state(|| {
        initial_finish(
            storage()
                .and_then(|s| s.get_item(FINISH_KEY).ok().flatten())
                .as_deref(),
            configured_finish.as_deref(),
        )
    });
    let xray = use_state(|| saved_flag(XRAY_KEY, false));
    let inner = use_state(|| saved_flag(INNER_KEY, true));
    let outline = use_state(|| saved_flag(OUTLINE_KEY, false));
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
    let flag_toggle = |state: UseStateHandle<bool>, key: &'static str| {
        Callback::from(move |_| {
            let next = !*state;
            save_flag(key, next);
            state.set(next)
        })
    };
    let pick_finish = {
        let finish = finish.clone();
        Callback::from(move |e: Event| {
            let value = e
                .target_unchecked_into::<web_sys::HtmlSelectElement>()
                .value();
            if let Some(s) = storage() {
                let _ = s.set_item(FINISH_KEY, &value);
            }
            finish.set(value)
        })
    };
    let options = if is_pcb {
        html! {<div class="viewer-options">
            <label class="viewer-option"><input type="checkbox" checked={*pours} onchange={toggle}/>{" Polygon pours"}</label>
        </div>}
    } else if is_model {
        html! {<div class="viewer-options">
            <label class="viewer-option">{"Pad finish "}
                <select onchange={pick_finish} aria-label="Pad surface finish">
                    { for FINISHES.iter().map(|(value, label)| html!{<option value={*value} selected={*value == finish.as_str()}>{*label}</option>}) }
                </select>
            </label>
            <label class="viewer-option"><input type="checkbox" checked={*xray} onchange={flag_toggle(xray.clone(), XRAY_KEY)}/>{" See-through board"}</label>
            <label class="viewer-option"><input type="checkbox" checked={*inner} onchange={flag_toggle(inner.clone(), INNER_KEY)}/>{" Inner copper"}</label>
            <label class="viewer-option"><input type="checkbox" checked={*outline} disabled={!*inner} onchange={flag_toggle(outline.clone(), OUTLINE_KEY)}/>{" Inner as outlines"}</label>
        </div>}
    } else {
        Html::default()
    };
    let view_options = ViewOptions {
        finish: (*finish).clone(),
        xray: *xray,
        inner: *inner,
        outline: *outline,
    };
    let body = if is_pcb {
        html! {<PcbView project={props.project.clone()} revision={props.revision.clone()} pours={*pours}/>}
    } else if is_model {
        html! {<ModelView project={props.project.clone()} revision={props.revision.clone()} active={props.active} options={Some(view_options)}/>}
    } else if is_schematic {
        html! {<SchematicView project={props.project.clone()} revision={props.revision.clone()}/>}
    } else {
        html! {<div class="lib-stage-msg">{format!("Unknown viewer kind: {}", props.kind)}</div>}
    };
    html! {<div class={classes!("viewer-card", is_pcb.then_some("rust-pcb"), is_model.then_some("rust-model"), is_schematic.then_some("rust-schematic"))}>{options}{body}</div>}
}

#[cfg(test)]
mod tests {
    use super::initial_finish;

    #[test]
    fn finish_prefers_the_persons_pick_then_the_project_then_the_board() {
        assert_eq!(initial_finish(Some("hasl"), Some("enig")), "hasl");
        assert_eq!(initial_finish(None, Some("enig")), "enig");
        assert_eq!(initial_finish(Some("bogus"), Some("enig")), "enig");
        assert_eq!(initial_finish(None, Some("bogus")), "board");
        assert_eq!(initial_finish(None, None), "board");
    }
}
