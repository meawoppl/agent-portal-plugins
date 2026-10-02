//! Typed lifecycle and selection boundary for the reified schematic scene.
//!
//! Drawing deliberately stays behind [`SceneViewport`]. The pastebom
//! `vector-view` draw/view API can replace that component without changing
//! fetch, revision, error, selection, or properties-panel behavior.

use std::rc::Rc;

use vector_view::{ItemId, Scene, SCENE_VERSION};
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;

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
            move |(project, _revision)| {
                state.set(LoadState::Loading);
                selected.set(None);
                let project = project.to_string();
                spawn_local(async move {
                    state.set(match crate::api::schematic(&project).await {
                        Ok(scene) if scene.version == SCENE_VERSION => {
                            LoadState::Ready(Rc::new(scene))
                        }
                        Ok(scene) => LoadState::Error(format!(
                            "Unsupported schematic scene version {} (viewer expects {})",
                            scene.version, SCENE_VERSION
                        )),
                        Err(error) => LoadState::Error(error),
                    });
                });
            },
        );
    }

    match &*state {
        LoadState::Loading => {
            html! { <div class="modelv-status">{"Loading schematic geometry…"}</div> }
        }
        LoadState::Error(error) => {
            html! { <div class="lib-stage-msg">{format!("Schematic viewer failed: {error}")}</div> }
        }
        LoadState::Ready(scene) => {
            let on_select = {
                let selected = selected.clone();
                Callback::from(move |id: Option<ItemId>| selected.set(id))
            };
            html! {
                <div class="schematicv">
                    <SceneViewport scene={scene.clone()} {on_select} />
                    if let Some(id) = *selected {
                        <SceneProperties scene={scene.clone()} item={id}
                            onclose={{let selected=selected.clone();Callback::from(move |_|selected.set(None))}} />
                    }
                </div>
            }
        }
    }
}

#[derive(Properties, PartialEq)]
struct SceneViewportProps {
    scene: Rc<Scene>,
    on_select: Callback<Option<ItemId>>,
}

/// Stable integration seam for `vector-view`'s forthcoming Canvas draw/view
/// API. This placeholder intentionally does not duplicate renderer geometry.
#[function_component(SceneViewport)]
fn scene_viewport(props: &SceneViewportProps) -> Html {
    let clear = {
        let on_select = props.on_select.clone();
        Callback::from(move |_| on_select.emit(None))
    };
    html! {
        <div class="native-viewer schematicv-stage" data-scene-version={props.scene.version.to_string()}
            data-items={props.scene.items.len().to_string()} onclick={clear}>
            <div class="modelv-status">
                {format!("Schematic geometry ready · {} items · {} nets · {} pages", props.scene.items.len(), props.scene.nets.len(), page_count(&props.scene))}
            </div>
        </div>
    }
}

fn page_count(scene: &Scene) -> usize {
    scene
        .meta
        .iter()
        .filter(|p| p.key.starts_with("page:") && p.key.ends_with(":file"))
        .count()
}

#[derive(Properties, PartialEq)]
struct ScenePropertiesProps {
    scene: Rc<Scene>,
    item: ItemId,
    onclose: Callback<()>,
}

#[function_component(SceneProperties)]
fn scene_properties(props: &ScenePropertiesProps) -> Html {
    let Some(item) = props.scene.items.iter().find(|item| item.id == props.item) else {
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
    html! {
        <aside class="pcbv-props" aria-label="Schematic selection properties">
            <div class="pcbv-props-head"><strong>{format!("Item {}", item.id)}</strong>
                <button onclick={Callback::from(move |_|onclose.emit(()))}>{"Close"}</button></div>
            <dl>{for rows.iter().map(|p|html!{<><dt>{&p.key}</dt><dd>{&p.value}</dd></>})}</dl>
        </aside>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn page_count_uses_canonical_metadata() {
        let mut scene = Scene::new(vector_view::SceneKind::Schematic, true);
        scene
            .meta
            .push(vector_view::Prop::new("page:/:file", "root.kicad_sch"));
        scene.meta.push(vector_view::Prop::new("revision", "abc"));
        assert_eq!(page_count(&scene), 1);
    }
}
