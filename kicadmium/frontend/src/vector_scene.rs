//! Reusable Yew/DOM host for pastebom's format-independent vector scenes.
use gloo_events::EventListener;
use std::{cell::RefCell, collections::HashSet, rc::Rc};
use vector_view::{
    hit::HitIndex,
    input::{Input, InputEvent, InputOutcome},
    render::{clear, draw_cached, PathCache},
    style::{Grid, Highlight, LayerVisibility, Overlay, Pass, Rgba, Theme, ViewState},
    view::View,
    BBox, ItemId, Scene,
};
use wasm_bindgen::JsCast;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement, PointerEvent, WheelEvent};
use yew::prelude::*;
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)] // Public command surface; consumers land independently.
pub enum VectorSceneCommand {
    Fit,
    Reset,
}

#[derive(Properties, PartialEq)]
pub struct VectorSceneCanvasProps {
    pub scene: Rc<Scene>,
    #[prop_or_default]
    pub selected: Option<ItemId>,
    #[prop_or_default]
    pub on_select: Callback<Option<ItemId>>,
    #[prop_or_default]
    pub visibility: LayerVisibility,
    #[prop_or_default]
    pub hidden_items: HashSet<ItemId>,
    #[prop_or_default]
    pub mirrored: bool,
    #[prop_or_default]
    pub passes: Option<Vec<Pass>>,
    #[prop_or_default]
    pub highlight: Highlight,
    #[prop_or_default]
    pub overlay: Option<Overlay>,
    /// Filled primitives rendered as outlines, including in the overlay pass.
    #[prop_or_default]
    pub outline_items: HashSet<ItemId>,
    #[prop_or_default]
    pub theme: Theme,
    #[prop_or(Some([17, 19, 29, 255]))]
    pub background: Option<Rgba>,
    #[prop_or_default]
    pub grid: Option<Grid>,
    #[prop_or_default]
    pub fit_bbox: Option<BBox>,
    #[prop_or(true)]
    pub fill: bool,
    #[prop_or_default]
    pub initial_view: Option<View>,
    #[prop_or_default]
    pub on_view: Callback<View>,
    #[prop_or_default]
    pub command: Option<VectorSceneCommand>,
}
struct Runtime {
    state: ViewState,
    input: Input,
    hit: HitIndex,
    cache: PathCache,
    fitted: bool,
    fit_bbox: Option<BBox>,
}
impl Runtime {
    fn new(s: &Scene, initial_view: Option<View>, mirrored: bool, fit_bbox: Option<BBox>) -> Self {
        let mut view = initial_view.unwrap_or_else(|| View::for_scene(s));
        view.mirrored = mirrored;
        Self {
            state: ViewState::new(view),
            input: Input::default(),
            hit: HitIndex::new(s),
            cache: PathCache::new(),
            fitted: initial_view.is_some(),
            fit_bbox,
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
        r.state
            .view
            .fit(r.fit_bbox.as_ref().unwrap_or(&s.bbox), w, h, 24.0);
        r.fitted = true
    }
    clear(
        &ctx,
        f64::from(c.width()),
        f64::from(c.height()),
        r.state.background,
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
    on_view: &Callback<View>,
) {
    match r.input.handle(&e, &mut r.state.view) {
        InputOutcome::None => {}
        InputOutcome::ViewChanged => {
            redraw(c, s, r);
            on_view.emit(r.state.view);
        }
        InputOutcome::Reset => {
            r.fitted = false;
            redraw(c, s, r);
            on_view.emit(r.state.view);
        }
        InputOutcome::Tap { x, y, .. } => {
            let p = r.state.view.to_world([x, y]);
            select.emit(
                r.hit
                    .hit_test_view(s, &r.state, p, 6.0 * r.state.view.world_per_px()),
            );
        }
    }
}
#[function_component(VectorSceneCanvas)]
pub fn vector_scene_canvas(props: &VectorSceneCanvasProps) -> Html {
    let canvas = use_node_ref();
    let runtime = use_mut_ref(|| {
        Runtime::new(
            &props.scene,
            props.initial_view,
            props.mirrored,
            props.fit_bbox,
        )
    });
    {
        let canvas = canvas.clone();
        let scene = props.scene.clone();
        let runtime = runtime.clone();
        let initial_view = props.initial_view;
        let mirrored = props.mirrored;
        let fit_bbox = props.fit_bbox;
        let on_view = props.on_view.clone();
        // A scene replacement rebuilds the runtime. Capture and apply every
        // declarative drawing prop here as well as in the config effect below:
        // unchanged props do not retrigger that second effect after a live
        // revision refresh.
        let visibility = props.visibility.clone();
        let hidden_items = props.hidden_items.clone();
        let passes = props.passes.clone();
        let highlight = props.highlight.clone();
        let selected = props.selected;
        let overlay = props.overlay.clone();
        let outline_items = props.outline_items.clone();
        let theme = props.theme.clone();
        let background = props.background;
        let grid = props.grid;
        use_effect_with(props.scene.clone(), move |_| {
            let mut fresh = Runtime::new(&scene, initial_view, mirrored, fit_bbox);
            fresh.state.visibility = visibility.clone();
            fresh.state.hidden_items = hidden_items.clone();
            fresh.state.passes = passes.clone();
            fresh.state.highlight = selected.map_or_else(
                || highlight.clone(),
                |id| Highlight::Items(HashSet::from([id])),
            );
            fresh.state.overlay = overlay.clone();
            fresh.state.outline_items = outline_items.clone();
            fresh.state.theme = theme.clone();
            fresh.state.background = background;
            fresh.state.grid = grid;
            *runtime.borrow_mut() = fresh;
            if let Some(c) = canvas.cast::<HtmlCanvasElement>() {
                let mut r = runtime.borrow_mut();
                redraw(&c, &scene, &mut r);
                on_view.emit(r.state.view);
            }
        });
    }
    {
        let canvas = canvas.clone();
        let scene = props.scene.clone();
        let runtime = runtime.clone();
        let on_view = props.on_view.clone();
        let mirrored = props.mirrored;
        use_effect_with(props.initial_view, move |initial| {
            if let Some(view) = initial {
                let mut r = runtime.borrow_mut();
                r.state.view = *view;
                r.state.view.mirrored = mirrored;
                r.fitted = true;
                if let Some(c) = canvas.cast::<HtmlCanvasElement>() {
                    redraw(&c, &scene, &mut r)
                }
                on_view.emit(r.state.view);
            }
        });
    }
    {
        let canvas = canvas.clone();
        let scene = props.scene.clone();
        let runtime = runtime.clone();
        let on_view = props.on_view.clone();
        let initial = props.initial_view;
        let mirrored = props.mirrored;
        use_effect_with(props.command.clone(), move |command| {
            let Some(command) = command else { return };
            let Some(c) = canvas.cast::<HtmlCanvasElement>() else {
                return;
            };
            let mut r = runtime.borrow_mut();
            match command {
                VectorSceneCommand::Fit => r.fitted = false,
                VectorSceneCommand::Reset => {
                    r.state.view = initial.unwrap_or_else(|| View::for_scene(&scene));
                    r.state.view.mirrored = mirrored;
                    r.fitted = initial.is_some();
                }
            }
            redraw(&c, &scene, &mut r);
            on_view.emit(r.state.view);
        });
    }
    {
        let canvas = canvas.clone();
        let scene = props.scene.clone();
        let runtime = runtime.clone();
        let config = (
            props.selected,
            props.visibility.clone(),
            props.hidden_items.clone(),
            props.mirrored,
            props.passes.clone(),
            props.highlight.clone(),
            props.overlay.clone(),
            props.outline_items.clone(),
            props.theme.clone(),
            props.background,
            props.grid,
            props.fit_bbox,
        );
        use_effect_with(config, move |config| {
            let (
                selected,
                visibility,
                hidden,
                mirrored,
                passes,
                highlight,
                overlay,
                outline_items,
                theme,
                background,
                grid,
                fit_bbox,
            ) = config;
            let mut r = runtime.borrow_mut();
            r.state.visibility = visibility.clone();
            r.state.hidden_items = hidden.clone();
            if let Some(c) = canvas.cast::<HtmlCanvasElement>() {
                r.state
                    .view
                    .set_mirrored(*mirrored, f64::from(c.client_width().max(1)));
            } else {
                r.state.view.mirrored = *mirrored;
            }
            r.state.passes = passes.clone();
            r.state.highlight = selected.map_or_else(
                || highlight.clone(),
                |id| Highlight::Items(HashSet::from([id])),
            );
            r.state.overlay = overlay.clone();
            r.state.outline_items = outline_items.clone();
            r.state.theme = theme.clone();
            r.state.background = *background;
            r.state.grid = *grid;
            r.fit_bbox = *fit_bbox;
            if let Some(c) = canvas.cast::<HtmlCanvasElement>() {
                redraw(&c, &scene, &mut r);
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
        props.on_view.clone(),
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
        props.on_view.clone(),
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
        props.on_view.clone(),
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
        let on_view = props.on_view.clone();
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
                &on_view,
            )
        })
    };
    let reset = {
        let canvas = canvas.clone();
        let scene = props.scene.clone();
        let runtime = runtime.clone();
        let on_view = props.on_view.clone();
        Callback::from(move |_| {
            if let Some(c) = canvas.cast::<HtmlCanvasElement>() {
                let mut r = runtime.borrow_mut();
                r.fitted = false;
                redraw(&c, &scene, &mut r);
                on_view.emit(r.state.view);
            }
        })
    };
    let style = if props.fill {
        "position:relative;height:100%;min-height:500px"
    } else {
        "position:relative"
    };
    html! {<div class="vector-scene" {style} data-scene-version={props.scene.version.to_string()}><canvas ref={canvas} class="native-viewer vector-scene-canvas" style="touch-action:none;display:block" aria-label="Interactive vector scene" onpointerdown={down} onpointermove={moved} onpointerup={up} onpointercancel={cancel} onwheel={wheel}/><button class="vector-scene-fit" style="position:absolute;right:12px;top:12px" onclick={reset}>{"Fit"}</button></div>}
}
fn pointer(
    canvas: NodeRef,
    scene: Rc<Scene>,
    runtime: Rc<RefCell<Runtime>>,
    select: Callback<Option<ItemId>>,
    on_view: Callback<View>,
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
        apply(
            &c,
            &scene,
            &mut runtime.borrow_mut(),
            make(e, p),
            &select,
            &on_view,
        )
    })
}
