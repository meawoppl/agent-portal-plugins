//! Reusable Yew/DOM host for pastebom's format-independent vector scenes.
use gloo_events::EventListener;
use std::{cell::RefCell, collections::HashSet, rc::Rc};
use vector_view::{
    hit::HitIndex,
    input::{Input, InputEvent, InputOutcome},
    render::{clear, draw_cached, PathCache},
    style::{Highlight, ViewState},
    view::View,
    ItemId, Scene,
};
use wasm_bindgen::JsCast;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement, PointerEvent, WheelEvent};
use yew::prelude::*;
#[derive(Properties, PartialEq)]
pub struct VectorSceneCanvasProps {
    pub scene: Rc<Scene>,
    #[prop_or_default]
    pub selected: Option<ItemId>,
    #[prop_or_default]
    pub on_select: Callback<Option<ItemId>>,
}
struct Runtime {
    state: ViewState,
    input: Input,
    hit: HitIndex,
    cache: PathCache,
    fitted: bool,
}
impl Runtime {
    fn new(s: &Scene) -> Self {
        Self {
            state: ViewState::new(View::for_scene(s)),
            input: Input::default(),
            hit: HitIndex::new(s),
            cache: PathCache::new(),
            fitted: false,
        }
    }
}
fn size(c: &HtmlCanvasElement) -> (f64, f64, f64) {
    let w = f64::from(c.client_width().max(1));
    let h = f64::from(c.client_height().max(1));
    let d = web_sys::window().map_or(1.0, |x| x.device_pixel_ratio().clamp(1.0, 3.0));
    c.set_width((w * d).round() as u32);
    c.set_height((h * d).round() as u32);
    (w, h, d)
}
fn redraw(c: &HtmlCanvasElement, s: &Scene, r: &mut Runtime) {
    let Ok(Some(raw)) = c.get_context("2d") else {
        return;
    };
    let Ok(ctx) = raw.dyn_into::<CanvasRenderingContext2d>() else {
        return;
    };
    let (w, h, d) = size(c);
    r.state.dpr = d;
    if !r.fitted {
        r.state.view.fit(&s.bbox, w, h, 24.0);
        r.fitted = true
    }
    clear(
        &ctx,
        f64::from(c.width()),
        f64::from(c.height()),
        Some([17, 19, 29, 255]),
    );
    draw_cached(s, &ctx, &r.state, &mut r.cache)
}
fn offset(c: &HtmlCanvasElement, x: i32, y: i32) -> [f64; 2] {
    let b = c.get_bounding_client_rect();
    [f64::from(x) - b.left(), f64::from(y) - b.top()]
}
fn apply(
    c: &HtmlCanvasElement,
    s: &Scene,
    r: &mut Runtime,
    e: InputEvent,
    select: &Callback<Option<ItemId>>,
) {
    match r.input.handle(&e, &mut r.state.view) {
        InputOutcome::None => {}
        InputOutcome::ViewChanged => redraw(c, s, r),
        InputOutcome::Reset => {
            r.fitted = false;
            redraw(c, s, r)
        }
        InputOutcome::Tap { x, y, .. } => {
            let p = r.state.view.to_world([x, y]);
            let visible = r.state.visibility.predicate(s);
            select.emit(
                r.hit
                    .hit_test(s, p, 6.0 * r.state.view.world_per_px(), &visible),
            );
        }
    }
}
#[function_component(VectorSceneCanvas)]
pub fn vector_scene_canvas(props: &VectorSceneCanvasProps) -> Html {
    let canvas = use_node_ref();
    let runtime = use_mut_ref(|| Runtime::new(&props.scene));
    {
        let canvas = canvas.clone();
        let scene = props.scene.clone();
        let runtime = runtime.clone();
        use_effect_with(props.scene.clone(), move |_| {
            *runtime.borrow_mut() = Runtime::new(&scene);
            if let Some(c) = canvas.cast::<HtmlCanvasElement>() {
                redraw(&c, &scene, &mut runtime.borrow_mut())
            }
        });
    }
    {
        let canvas = canvas.clone();
        let scene = props.scene.clone();
        let runtime = runtime.clone();
        use_effect_with(props.selected, move |selected| {
            let mut r = runtime.borrow_mut();
            r.state.highlight =
                selected.map_or(Highlight::None, |id| Highlight::Items(HashSet::from([id])));
            r.state.theme.highlight = Some([247, 118, 142, 255]);
            if let Some(c) = canvas.cast::<HtmlCanvasElement>() {
                redraw(&c, &scene, &mut r)
            }
        });
    }
    {
        let canvas = canvas.clone();
        let scene = props.scene.clone();
        let runtime = runtime.clone();
        use_effect_with((), move |_| {
            let listener = web_sys::window().map(|w| {
                EventListener::new(&w, "resize", move |_| {
                    if let Some(c) = canvas.cast::<HtmlCanvasElement>() {
                        redraw(&c, &scene, &mut runtime.borrow_mut())
                    }
                })
            });
            move || drop(listener)
        });
    }
    let down = pointer(
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
    let moved = pointer(
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
    let up = pointer(
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
    let cancel = {
        let runtime = runtime.clone();
        Callback::from(move |e: PointerEvent| {
            let mut r = runtime.borrow_mut();
            let mut v = r.state.view;
            let _ = r
                .input
                .handle(&InputEvent::PointerCancel { id: e.pointer_id() }, &mut v);
            r.state.view = v;
        })
    };
    let wheel = {
        let canvas = canvas.clone();
        let scene = props.scene.clone();
        let runtime = runtime.clone();
        let select = props.on_select.clone();
        Callback::from(move |e: WheelEvent| {
            e.prevent_default();
            let Some(c) = canvas.cast::<HtmlCanvasElement>() else {
                return;
            };
            let p = offset(&c, e.client_x(), e.client_y());
            apply(
                &c,
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
                &select,
            )
        })
    };
    let reset = {
        let canvas = canvas.clone();
        let scene = props.scene.clone();
        let runtime = runtime.clone();
        Callback::from(move |_| {
            if let Some(c) = canvas.cast::<HtmlCanvasElement>() {
                let mut r = runtime.borrow_mut();
                r.fitted = false;
                redraw(&c, &scene, &mut r)
            }
        })
    };
    html! {<div class="vector-scene" style="position:relative;height:100%;min-height:500px" data-scene-version={props.scene.version.to_string()}><canvas ref={canvas} class="native-viewer vector-scene-canvas" style="touch-action:none;display:block" aria-label="Interactive vector scene" onpointerdown={down} onpointermove={moved} onpointerup={up} onpointercancel={cancel} onwheel={wheel}/><button class="vector-scene-fit" style="position:absolute;right:12px;top:12px" onclick={reset}>{"Fit"}</button></div>}
}
fn pointer(
    canvas: NodeRef,
    scene: Rc<Scene>,
    runtime: Rc<RefCell<Runtime>>,
    select: Callback<Option<ItemId>>,
    make: impl Fn(PointerEvent, [f64; 2]) -> InputEvent + 'static,
) -> Callback<PointerEvent> {
    Callback::from(move |e: PointerEvent| {
        e.prevent_default();
        let Some(c) = canvas.cast::<HtmlCanvasElement>() else {
            return;
        };
        if e.type_() == "pointerdown" {
            let _ = c.set_pointer_capture(e.pointer_id());
        }
        let p = offset(&c, e.client_x(), e.client_y());
        apply(&c, &scene, &mut runtime.borrow_mut(), make(e, p), &select)
    })
}
