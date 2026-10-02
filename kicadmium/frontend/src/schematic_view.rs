//! Typed lifecycle and selection boundary for the reified schematic scene.
//!
//! Drawing and interaction are supplied by pastebom's `vector-view`; Yew owns
//! only DOM lifecycle, revision loading and the properties panel.

use std::{cell::RefCell, collections::HashSet, rc::Rc};

use gloo_events::EventListener;
use vector_view::{
    hit::HitIndex,
    input::{Input, InputEvent, InputOutcome},
    render::{clear, draw_cached, PathCache},
    style::{Highlight, ViewState},
    view::View,
    ItemId, Scene, SCENE_VERSION,
};
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement, PointerEvent, WheelEvent};
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
                    <SceneViewport scene={scene.clone()} selected={*selected} {on_select} />
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
    selected: Option<ItemId>,
    on_select: Callback<Option<ItemId>>,
}

struct Runtime {
    state: ViewState,
    input: Input,
    hit: HitIndex,
    cache: PathCache,
    fitted: bool,
}

impl Runtime {
    fn new(scene: &Scene) -> Self {
        Self {
            state: ViewState::new(View::for_scene(scene)),
            input: Input::default(),
            hit: HitIndex::new(scene),
            cache: PathCache::new(),
            fitted: false,
        }
    }
}

fn canvas_size(canvas: &HtmlCanvasElement) -> (f64, f64, f64) {
    let width = f64::from(canvas.client_width().max(1));
    let height = f64::from(canvas.client_height().max(1));
    let dpr = web_sys::window().map_or(1.0, |w| w.device_pixel_ratio().clamp(1.0, 3.0));
    canvas.set_width((width * dpr).round() as u32);
    canvas.set_height((height * dpr).round() as u32);
    (width, height, dpr)
}

fn redraw(canvas: &HtmlCanvasElement, scene: &Scene, runtime: &mut Runtime) {
    let Ok(Some(raw)) = canvas.get_context("2d") else {
        return;
    };
    let Ok(ctx) = raw.dyn_into::<CanvasRenderingContext2d>() else {
        return;
    };
    let (width, height, dpr) = canvas_size(canvas);
    runtime.state.dpr = dpr;
    if !runtime.fitted {
        runtime.state.view.fit(&scene.bbox, width, height, 24.0);
        runtime.fitted = true;
    }
    clear(
        &ctx,
        f64::from(canvas.width()),
        f64::from(canvas.height()),
        Some([17, 19, 29, 255]),
    );
    draw_cached(scene, &ctx, &runtime.state, &mut runtime.cache);
}

fn offset(canvas: &HtmlCanvasElement, client_x: i32, client_y: i32) -> [f64; 2] {
    let bounds = canvas.get_bounding_client_rect();
    [
        f64::from(client_x) - bounds.left(),
        f64::from(client_y) - bounds.top(),
    ]
}

fn apply(
    canvas: &HtmlCanvasElement,
    scene: &Scene,
    runtime: &mut Runtime,
    event: InputEvent,
    on_select: &Callback<Option<ItemId>>,
) {
    match runtime.input.handle(&event, &mut runtime.state.view) {
        InputOutcome::None => {}
        InputOutcome::ViewChanged => redraw(canvas, scene, runtime),
        InputOutcome::Reset => {
            runtime.fitted = false;
            redraw(canvas, scene, runtime);
        }
        InputOutcome::Tap { x, y, .. } => {
            let point = runtime.state.view.to_world([x, y]);
            let visible = runtime.state.visibility.predicate(scene);
            let hit = runtime.hit.hit_test(
                scene,
                point,
                6.0 * runtime.state.view.world_per_px(),
                &visible,
            );
            on_select.emit(hit);
        }
    }
}

/// Thin Yew/DOM host around pastebom's reusable vector-view engine.
#[function_component(SceneViewport)]
fn scene_viewport(props: &SceneViewportProps) -> Html {
    let canvas = use_node_ref();
    let runtime = use_mut_ref(|| Runtime::new(&props.scene));
    {
        let canvas = canvas.clone();
        let scene = props.scene.clone();
        let runtime = runtime.clone();
        use_effect_with(props.scene.clone(), move |_| {
            *runtime.borrow_mut() = Runtime::new(&scene);
            if let Some(canvas) = canvas.cast::<HtmlCanvasElement>() {
                redraw(&canvas, &scene, &mut runtime.borrow_mut());
            }
        });
    }
    {
        let canvas = canvas.clone();
        let scene = props.scene.clone();
        let runtime = runtime.clone();
        use_effect_with(props.selected, move |selected| {
            let mut rt = runtime.borrow_mut();
            rt.state.highlight =
                selected.map_or(Highlight::None, |id| Highlight::Items(HashSet::from([id])));
            rt.state.theme.highlight = Some([247, 118, 142, 255]);
            if let Some(canvas) = canvas.cast::<HtmlCanvasElement>() {
                redraw(&canvas, &scene, &mut rt);
            }
        });
    }
    {
        let canvas = canvas.clone();
        let scene = props.scene.clone();
        let runtime = runtime.clone();
        use_effect_with((), move |_| {
            let listener = web_sys::window().map(|window| {
                EventListener::new(&window, "resize", move |_| {
                    if let Some(canvas) = canvas.cast::<HtmlCanvasElement>() {
                        redraw(&canvas, &scene, &mut runtime.borrow_mut());
                    }
                })
            });
            move || drop(listener)
        });
    }

    let pointer_down = pointer_handler(
        canvas.clone(),
        props.scene.clone(),
        runtime.clone(),
        props.on_select.clone(),
        |e, p| InputEvent::PointerDown {
            id: e.pointer_id(),
            x: p[0],
            y: p[1],
            button: e.button(),
            time_ms: e.time_stamp(),
        },
    );
    let pointer_move = pointer_handler(
        canvas.clone(),
        props.scene.clone(),
        runtime.clone(),
        props.on_select.clone(),
        |e, p| InputEvent::PointerMove {
            id: e.pointer_id(),
            x: p[0],
            y: p[1],
        },
    );
    let pointer_up = pointer_handler(
        canvas.clone(),
        props.scene.clone(),
        runtime.clone(),
        props.on_select.clone(),
        |e, p| InputEvent::PointerUp {
            id: e.pointer_id(),
            x: p[0],
            y: p[1],
            button: e.button(),
            time_ms: e.time_stamp(),
        },
    );
    let pointer_cancel = {
        let runtime = runtime.clone();
        Callback::from(move |e: PointerEvent| {
            let mut rt = runtime.borrow_mut();
            let mut view = rt.state.view;
            let _ = rt
                .input
                .handle(&InputEvent::PointerCancel { id: e.pointer_id() }, &mut view);
            rt.state.view = view;
        })
    };
    let wheel = {
        let canvas = canvas.clone();
        let scene = props.scene.clone();
        let runtime = runtime.clone();
        let on_select = props.on_select.clone();
        Callback::from(move |e: WheelEvent| {
            e.prevent_default();
            let Some(canvas) = canvas.cast::<HtmlCanvasElement>() else {
                return;
            };
            let p = offset(&canvas, e.client_x(), e.client_y());
            apply(
                &canvas,
                &scene,
                &mut runtime.borrow_mut(),
                InputEvent::Wheel {
                    x: p[0],
                    y: p[1],
                    dx: e.delta_x(),
                    dy: e.delta_y(),
                    delta_mode: e.delta_mode(),
                    ctrl: e.ctrl_key(),
                },
                &on_select,
            );
        })
    };
    let reset = {
        let canvas = canvas.clone();
        let scene = props.scene.clone();
        let runtime = runtime.clone();
        Callback::from(move |_| {
            if let Some(canvas) = canvas.cast::<HtmlCanvasElement>() {
                let mut rt = runtime.borrow_mut();
                rt.fitted = false;
                redraw(&canvas, &scene, &mut rt);
            }
        })
    };
    html! {
        <div class="schematicv-stage" style="position:relative;height:100%;min-height:500px"
            data-scene-version={props.scene.version.to_string()} data-pages={page_count(&props.scene).to_string()}>
            <canvas ref={canvas} class="native-viewer schematicv-canvas" style="touch-action:none;display:block" aria-label="Interactive schematic"
                onpointerdown={pointer_down} onpointermove={pointer_move} onpointerup={pointer_up}
                onpointercancel={pointer_cancel} onwheel={wheel}/>
            <button class="schematicv-fit" style="position:absolute;right:12px;top:12px" onclick={reset}>{"Fit"}</button>
        </div>
    }
}

fn pointer_handler(
    canvas: NodeRef,
    scene: Rc<Scene>,
    runtime: Rc<RefCell<Runtime>>,
    on_select: Callback<Option<ItemId>>,
    make: impl Fn(PointerEvent, [f64; 2]) -> InputEvent + 'static,
) -> Callback<PointerEvent> {
    Callback::from(move |e: PointerEvent| {
        e.prevent_default();
        let Some(canvas) = canvas.cast::<HtmlCanvasElement>() else {
            return;
        };
        if e.type_() == "pointerdown" {
            let _ = canvas.set_pointer_capture(e.pointer_id());
        }
        let p = offset(&canvas, e.client_x(), e.client_y());
        apply(
            &canvas,
            &scene,
            &mut runtime.borrow_mut(),
            make(e, p),
            &on_select,
        );
    })
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
