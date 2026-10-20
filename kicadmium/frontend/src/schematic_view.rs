//! Schematic tab: fetches the backend's schematic scene for the project and
//! revision and hands it to `vector_view_yew::SchematicView`, which draws one
//! sheet at a time with kicadmium's own chrome (search, sheet picker, parent
//! sheet, layers, warnings, properties, net highlight).
use std::{cell::Cell, rc::Rc};
use vector_view::{Scene, SCENE_VERSION};
use vector_view_yew::Reveal;
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;

#[derive(Properties, PartialEq)]
pub struct SchematicViewProps {
    pub project: AttrValue,
    pub revision: AttrValue,
    /// Sheet to open first (`?sheet=` deep link).
    #[prop_or_default]
    pub sheet: Option<AttrValue>,
    /// Symbol reference to select and frame once loaded (`?ref=` deep link).
    #[prop_or_default]
    pub reveal: Option<AttrValue>,
    /// Show a symbol on the PCB tab; offered only when the project has a board.
    #[prop_or_default]
    pub on_cross_probe: Option<Callback<String>>,
}

#[derive(Clone, PartialEq)]
enum LoadState {
    Loading,
    Ready(Rc<Scene>),
    Error(String),
}

#[component(SchematicView)]
pub fn schematic_view(props: &SchematicViewProps) -> Html {
    let state = use_state(|| LoadState::Loading);
    // The open sheet lives here so it survives the remount on every refetch.
    let page = use_state(|| props.sheet.as_ref().map(|s| s.to_string()));
    // A `?ref=` deep link is applied to the first scene only, not again on
    // every live refresh.
    let revealed = use_mut_ref(|| Cell::new(false));
    {
        let state = state.clone();
        use_effect_with(
            (props.project.clone(), props.revision.clone()),
            move |(project, _)| {
                state.set(LoadState::Loading);
                let project = project.to_string();
                let token = crate::request::RequestToken::default();
                let task_token = token.clone();
                spawn_local(async move {
                    let response = crate::api::schematic(&project).await;
                    if !task_token.current() {
                        return;
                    }
                    state.set(match response {
                        Ok(scene) if scene.version == SCENE_VERSION => {
                            LoadState::Ready(Rc::new(scene))
                        }
                        Ok(scene) => LoadState::Error(format!(
                            "Unsupported schematic scene version {} (viewer expects {})",
                            scene.version, SCENE_VERSION
                        )),
                        Err(e) => LoadState::Error(e),
                    });
                });
                move || token.cancel()
            },
        );
    }
    match &*state {
        LoadState::Loading => {
            html! {<div class="modelv-status">{"Loading schematic geometry…"}</div>}
        }
        LoadState::Error(e) => {
            html! {<div class="lib-stage-msg">{format!("Schematic viewer failed: {e}")}</div>}
        }
        LoadState::Ready(scene) => {
            let reveal = props
                .reveal
                .as_ref()
                .filter(|_| !revealed.borrow().replace(true))
                .map(|reference| Reveal {
                    seq: 0,
                    reference: reference.to_string(),
                });
            let on_page = {
                let page = page.clone();
                Callback::from(move |p: String| page.set(Some(p)))
            };
            html! {
                <vector_view_yew::SchematicView
                    scene={scene.clone()}
                    page={(*page).clone().map(AttrValue::from)}
                    {on_page}
                    {reveal}
                    on_cross_probe={props.on_cross_probe.clone()}
                />
            }
        }
    }
}
