//! Typed lifecycle and properties panel for the reified schematic scene.
use crate::vector_scene::VectorSceneCanvas;
use std::{collections::HashSet, rc::Rc};
use vector_view::{BBox, ItemId, Role, Scene, SCENE_VERSION};
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
/// Sheets of a hierarchical design in traversal order: `(page path, file)`,
/// from the scene's `page:{path}:file` metadata.
fn pages(scene: &Scene) -> Vec<(String, String)> {
    scene
        .meta
        .iter()
        .filter_map(|p| {
            let path = p.key.strip_prefix("page:")?.strip_suffix(":file")?;
            Some((path.to_owned(), p.value.clone()))
        })
        .collect()
}
/// The page's drawing-sheet area (`page:{path}:size` = "w,h" in mm).
fn page_box(scene: &Scene, page: &str) -> Option<BBox> {
    let key = format!("page:{page}:size");
    let size = scene.meta.iter().find(|p| p.key == key)?;
    let (w, h) = size.value.split_once(',')?;
    Some(BBox {
        min: [0.0, 0.0],
        max: [w.parse().ok()?, h.parse().ok()?],
    })
}
/// Items that belong to other pages than `page`.
fn off_page(scene: &Scene, page: &str) -> HashSet<ItemId> {
    scene
        .items
        .iter()
        .filter(|i| i.props.iter().any(|p| p.key == "page" && p.value != page))
        .map(|i| i.id)
        .collect()
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
    let page = use_state(|| "/".to_owned());
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
            let all = pages(scene);
            // Keep the open sheet across live revisions while it still exists.
            let current = if all.iter().any(|(p, _)| *p == *page) {
                (*page).clone()
            } else {
                all.first()
                    .map(|(p, _)| p.clone())
                    .unwrap_or_else(|| "/".into())
            };
            let hidden_items = off_page(scene, &current);
            let fit_bbox = page_box(scene, &current);
            let on_select = {
                let selected = selected.clone();
                Callback::from(move |id: Option<ItemId>| selected.set(id))
            };
            let open = {
                let page = page.clone();
                let selected = selected.clone();
                Callback::from(move |p: String| {
                    selected.set(None);
                    page.set(p)
                })
            };
            let picker = if all.len() > 1 {
                let open = open.clone();
                let onchange = Callback::from(move |e: Event| {
                    open.emit(
                        e.target_unchecked_into::<web_sys::HtmlSelectElement>()
                            .value(),
                    )
                });
                html! {<select class="schematicv-pages" aria-label="Schematic sheet" {onchange}>{for all.iter().enumerate().map(|(i,(p,file))|html!{<option value={p.clone()} selected={*p==current}>{format!("{}. {} ({file})",i+1,p)}</option>})}</select>}
            } else {
                Html::default()
            };
            html! {<div class="schematicv">{picker}<VectorSceneCanvas key={current.clone()} scene={scene.clone()} selected={*selected} {on_select} {hidden_items} {fit_bbox} theme={schematic_theme()} background={Some(SCHEMATIC_BACKGROUND)}/>{if let Some(id)=*selected{html!{<SceneProperties scene={scene.clone()} item={id} onclose={{let selected=selected.clone();Callback::from(move |_|selected.set(None))}} onopen={open.clone()}/>}}else{Html::default()}}</div>}
        }
    }
}
#[derive(Properties, PartialEq)]
struct ScenePropertiesProps {
    scene: Rc<Scene>,
    item: ItemId,
    onclose: Callback<()>,
    /// Open a child sheet (hierarchical sheet frames).
    onopen: Callback<String>,
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
    let child = (item.role == Role::SheetFrame)
        .then(|| item.group.and_then(|id| props.scene.group(id)))
        .flatten()
        .and_then(|g| g.props.iter().find(|p| p.key == "child_page"))
        .map(|p| p.value.clone());
    let open = child.map(|page| {
        let onopen = props.onopen.clone();
        html! {<button onclick={Callback::from(move |_|onopen.emit(page.clone()))}>{"Open sheet"}</button>}
    });
    html! {<aside class="pcbv-props" aria-label="Schematic selection properties"><div class="pcbv-props-head"><strong>{format!("Item {}",item.id)}</strong>{open.unwrap_or_default()}<button onclick={Callback::from(move |_|onclose.emit(()))}>{"Close"}</button></div><dl>{for rows.iter().map(|p|html!{<><dt>{&p.key}</dt><dd>{&p.value}</dd></>})}</dl></aside>}
}
