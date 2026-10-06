//! Typed lifecycle and properties panel for the reified schematic scene.
use crate::vector_scene::VectorSceneCanvas;
use std::rc::Rc;
use vector_view::{ItemId, Scene, SCENE_VERSION};
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;
/// Portal (Tokyo Night) canvas background; the scene's layer colours carry
/// the rest of the default palette (see backend `schematic_view::palette`).
const SCHEMATIC_BACKGROUND: vector_view::style::Rgba = [0x1a, 0x1b, 0x26, 255];
/// Selected items are drawn in Tokyo Night orange while the rest dims.
const SCHEMATIC_SELECTION: vector_view::style::Rgba = [0xff, 0x9e, 0x64, 255];
fn schematic_theme() -> vector_view::style::Theme {
    vector_view::style::Theme {
        highlight: Some(SCHEMATIC_SELECTION),
        ..Default::default()
    }
}
#[derive(Properties, PartialEq)]
pub struct SchematicViewProps {
    pub project: AttrValue,
    pub revision: AttrValue,
}
#[derive(Clone, PartialEq)]
enum LoadState {
    Loading,
    Ready(Rc<Scene>),
    Error(String),
}
#[function_component(SchematicView)]
pub fn schematic_view(props: &SchematicViewProps) -> Html {
    let state = use_state(|| LoadState::Loading);
    let selected = use_state(|| None::<ItemId>);
    {
        let state = state.clone();
        let selected = selected.clone();
        use_effect_with(
            (props.project.clone(), props.revision.clone()),
            move |(project, _)| {
                state.set(LoadState::Loading);
                selected.set(None);
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
            let on_select = {
                let selected = selected.clone();
                Callback::from(move |id: Option<ItemId>| selected.set(id))
            };
            html! {<div class="schematicv"><VectorSceneCanvas scene={scene.clone()} selected={*selected} {on_select} theme={schematic_theme()} background={Some(SCHEMATIC_BACKGROUND)}/>{if let Some(id)=*selected{html!{<SceneProperties scene={scene.clone()} item={id} onclose={{let selected=selected.clone();Callback::from(move |_|selected.set(None))}}/>}}else{Html::default()}}</div>}
        }
    }
}
#[derive(Properties, PartialEq)]
struct ScenePropertiesProps {
    scene: Rc<Scene>,
    item: ItemId,
    onclose: Callback<()>,
}
#[function_component(SceneProperties)]
fn scene_properties(props: &ScenePropertiesProps) -> Html {
    let Some(item) = props.scene.items.iter().find(|x| x.id == props.item) else {
        return Html::default();
    };
    let mut rows = item.props.clone();
    rows.push(vector_view::Prop::new("role", format!("{:?}", item.role)));
    if let Some(net) = item.net.and_then(|id| props.scene.net(id)) {
        rows.push(vector_view::Prop::new("net", &net.name));
    }
    if let Some(group) = item.group.and_then(|id| props.scene.group(id)) {
        rows.push(vector_view::Prop::new("group", &group.label));
    }
    let onclose = props.onclose.clone();
    html! {<aside class="pcbv-props" aria-label="Schematic selection properties"><div class="pcbv-props-head"><strong>{format!("Item {}",item.id)}</strong><button onclick={Callback::from(move |_|onclose.emit(()))}>{"Close"}</button></div><dl>{for rows.iter().map(|p|html!{<><dt>{&p.key}</dt><dd>{&p.value}</dd></>})}</dl></aside>}
}
