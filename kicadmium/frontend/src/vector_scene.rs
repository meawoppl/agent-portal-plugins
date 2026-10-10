//! Reusable Yew/DOM host for pastebom's format-independent vector scenes.
use gloo_events::EventListener;
use std::{cell::RefCell, collections::HashSet, rc::Rc};
use vector_view::{
    hit::HitIndex,
    input::{InputEvent, InputOutcome},
    render::{clear, draw_cached, PathCache},
    style::{Grid, Highlight, LayerVisibility, Overlay, Pass, Rgba, Theme, ViewState},
    view::View,
    BBox, ItemId, Scene,
};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use web_sys::{
    CanvasRenderingContext2d, HtmlCanvasElement, PointerEvent, ResizeObserver, WheelEvent,
};
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
    /// Share of the limiting viewport side the fit box fills (KiCanvas-style
    /// framing); `None` fits it to the padded viewport.
    #[prop_or_default]
    pub fit_fill: Option<f64>,
    #[prop_or(true)]
    pub fill: bool,
    #[prop_or_default]
    pub initial_view: Option<View>,
    #[prop_or_default]
    pub on_view: Callback<View>,
    #[prop_or_default]
    pub command: Option<VectorSceneCommand>,
}
/// Padding left around a fit, in CSS pixels.
const FIT_PAD: f64 = 24.0;
/// A restored camera showing the fit box smaller than this (CSS pixels) is
/// treated as stale and replaced by a fit.
const MIN_VISIBLE_PX: f64 = 8.0;

struct Runtime {
    state: ViewState,
    input: crate::view_input::ViewInput,
    hit: HitIndex,
    cache: PathCache,
    /// The view is valid for drawing (fitted, restored or user-driven).
    fitted: bool,
    /// The view came from an automatic fit; resizes refit it.
    auto_fit: bool,
    /// A restored camera still has to be checked against a real viewport.
    unchecked: bool,
    fit_bbox: Option<BBox>,
    fit_fill: Option<f64>,
    /// Last laid-out canvas size in CSS pixels.
    size: (f64, f64),
}
impl Runtime {
    fn new(
        s: &Scene,
        initial_view: Option<View>,
        mirrored: bool,
        fit_bbox: Option<BBox>,
        fit_fill: Option<f64>,
    ) -> Self {
        let mut view = initial_view.unwrap_or_else(|| View::for_scene(s));
        view.mirrored = mirrored;
        Self {
            state: ViewState::new(view),
            input: crate::view_input::ViewInput::default(),
            hit: HitIndex::new(s),
            cache: PathCache::new(),
            fitted: initial_view.is_some(),
            auto_fit: false,
            unchecked: initial_view.is_some(),
            fit_bbox,
            fit_fill,
            size: (0.0, 0.0),
        }
    }
    /// Use `view` as given (a restored or toolbar camera), validated on draw.
    fn restore(&mut self, view: View) {
        self.state.view = view;
        self.fitted = true;
        self.auto_fit = false;
        self.unchecked = true;
    }
    /// Fit on the next draw that has a real viewport.
    fn refit(&mut self) {
        self.fitted = false;
    }
}

/// Fit `bbox` into a `w` x `h` viewport; with `fill`, the box covers that
/// share of the limiting side instead of the padded viewport.
fn fit_view(view: &mut View, bbox: &BBox, w: f64, h: f64, fill: Option<f64>) {
    view.fit(bbox, w, h, FIT_PAD);
    if let (Some(fill), false) = (fill, bbox.is_empty()) {
        let (bw, bh) = (bbox.width().max(1e-6), bbox.height().max(1e-6));
        let want = (w / bw).min(h / bh) * fill;
        let padded = ((w - 2.0 * FIT_PAD).max(1.0) / bw).min((h - 2.0 * FIT_PAD).max(1.0) / bh);
        view.zoom_at(w / 2.0, h / 2.0, (want / padded).clamp(0.05, 1.0));
    }
}

/// Whether `view` shows `bbox` at a usable size somewhere in the viewport.
fn frames(view: &View, bbox: &BBox, w: f64, h: f64) -> bool {
    if bbox.is_empty() {
        return true;
    }
    let corners = [
        bbox.min,
        bbox.max,
        [bbox.min[0], bbox.max[1]],
        [bbox.max[0], bbox.min[1]],
    ]
    .map(|p| view.to_screen(p));
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for [x, y] in corners {
        (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
    }
    let finite = [x0, y0, x1, y1].iter().all(|v| v.is_finite());
    finite && (x1 - x0).max(y1 - y0) >= MIN_VISIBLE_PX && x1 > 0.0 && y1 > 0.0 && x0 < w && y0 < h
}
/// Size the backing store; `None` while the canvas is not laid out (a hidden
/// tab or a docked pane that is still opening).
fn size(c: &HtmlCanvasElement) -> Option<(f64, f64, f64)> {
    if c.client_width() <= 0 || c.client_height() <= 0 {
        return None;
    }
    let w = f64::from(c.client_width());
    let h = f64::from(c.client_height());
    let d = web_sys::window().map_or(1.0, |x| x.device_pixel_ratio().clamp(1.0, 3.0));
    c.set_width((w * d).round() as u32);
    c.set_height((h * d).round() as u32);
    Some((w, h, d))
}
/// Draw the scene; returns whether a view valid for this viewport exists
/// (and so may be reported and persisted).
fn redraw(c: &HtmlCanvasElement, s: &Scene, r: &mut Runtime) -> bool {
    let Ok(Some(raw)) = c.get_context("2d") else {
        return false;
    };
    let Ok(ctx) = raw.dyn_into::<CanvasRenderingContext2d>() else {
        return false;
    };
    // Never fit against a collapsed canvas: a 1x1 fit shrinks the scene to a
    // dot, and the host would persist that camera for every later load.
    let Some((w, h, d)) = size(c) else {
        return false;
    };
    let resized = r.size != (w, h);
    r.size = (w, h);
    r.state.dpr = d;
    let bbox = r.fit_bbox.unwrap_or(s.bbox);
    if r.unchecked {
        r.unchecked = false;
        if !frames(&r.state.view, &bbox, w, h) {
            r.fitted = false;
        }
    }
    if !r.fitted || (r.auto_fit && resized) {
        fit_view(&mut r.state.view, &bbox, w, h, r.fit_fill);
        r.fitted = true;
        r.auto_fit = true;
    }
    clear(
        &ctx,
        f64::from(c.width()),
        f64::from(c.height()),
        r.state.background,
    );
    draw_cached(s, &ctx, &r.state, &mut r.cache);
    publish_view(c, &r.state.view);
    true
}

/// Attribute on the canvas carrying the current camera, so components that
/// only see the DOM (the annotation composer) can map CSS pixels to scene
/// millimetres with [`view_of`].
const VIEW_ATTR: &str = "data-view";

fn publish_view(c: &HtmlCanvasElement, v: &View) {
    let _ = c.set_attribute(VIEW_ATTR, &encode_view(v));
}

fn encode_view(v: &View) -> String {
    format!(
        "{} {} {} {} {} {}",
        v.scale, v.tx, v.ty, v.rotation, v.mirrored as u8, v.y_up as u8
    )
}

fn decode_view(text: &str) -> Option<View> {
    let mut parts = text.split(' ');
    let mut next = || parts.next()?.parse::<f64>().ok();
    let view = View {
        scale: next()?,
        tx: next()?,
        ty: next()?,
        rotation: next()?,
        mirrored: next()? != 0.0,
        y_up: next()? != 0.0,
    };
    (view.scale.is_finite() && view.scale > 0.0).then_some(view)
}

/// The camera last drawn on a `VectorSceneCanvas` canvas, if any.
pub fn view_of(c: &HtmlCanvasElement) -> Option<View> {
    decode_view(&c.get_attribute(VIEW_ATTR)?)
}
/// Redraw and report the view when it is valid for the current viewport.
fn redraw_report(c: &HtmlCanvasElement, s: &Scene, r: &mut Runtime, on_view: &Callback<View>) {
    if redraw(c, s, r) {
        on_view.emit(r.state.view);
    }
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
            r.auto_fit = false;
            redraw_report(c, s, r, on_view);
        }
        InputOutcome::Reset => {
            r.refit();
            redraw_report(c, s, r, on_view);
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
            props.fit_fill,
        )
    });
    {
        let canvas = canvas.clone();
        let scene = props.scene.clone();
        let runtime = runtime.clone();
        let initial_view = props.initial_view;
        let mirrored = props.mirrored;
        let fit_bbox = props.fit_bbox;
        let fit_fill = props.fit_fill;
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
            let mut fresh = Runtime::new(&scene, initial_view, mirrored, fit_bbox, fit_fill);
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
                redraw_report(&c, &scene, &mut runtime.borrow_mut(), &on_view);
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
                r.restore(View { mirrored, ..*view });
                if let Some(c) = canvas.cast::<HtmlCanvasElement>() {
                    redraw_report(&c, &scene, &mut r, &on_view);
                }
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
                VectorSceneCommand::Fit => r.refit(),
                VectorSceneCommand::Reset => match initial {
                    Some(view) => r.restore(View { mirrored, ..view }),
                    None => {
                        r.state.view = View {
                            mirrored,
                            ..View::for_scene(&scene)
                        };
                        r.refit();
                    }
                },
            }
            redraw_report(&c, &scene, &mut r, &on_view);
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
            (props.fit_bbox, props.fit_fill),
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
                (fit_bbox, fit_fill),
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
            r.fit_fill = *fit_fill;
            if let Some(c) = canvas.cast::<HtmlCanvasElement>() {
                redraw(&c, &scene, &mut r);
            }
        });
    }
    {
        let canvas = canvas.clone();
        let scene = props.scene.clone();
        let runtime = runtime.clone();
        let on_view = props.on_view.clone();
        // Track the canvas box itself: docked panes and tabs resize (or first
        // lay out) without a window resize.
        use_effect_with(props.scene.clone(), move |_| {
            let element = canvas.cast::<HtmlCanvasElement>();
            let callback = Closure::<dyn FnMut()>::new({
                let canvas = canvas.clone();
                move || {
                    if let Some(c) = canvas.cast::<HtmlCanvasElement>() {
                        let Ok(mut r) = runtime.try_borrow_mut() else {
                            return;
                        };
                        let before = (r.state.view, r.size);
                        if redraw(&c, &scene, &mut r) && (r.state.view, r.size) != before {
                            on_view.emit(r.state.view);
                        }
                    }
                }
            });
            let observer = ResizeObserver::new(callback.as_ref().unchecked_ref()).ok();
            if let (Some(observer), Some(element)) = (&observer, &element) {
                observer.observe(element);
            }
            let listener = web_sys::window().map(|w| {
                let callback = callback
                    .as_ref()
                    .unchecked_ref::<js_sys::Function>()
                    .clone();
                EventListener::new(&w, "resize", move |_| {
                    let _ = callback.call0(&wasm_bindgen::JsValue::NULL);
                })
            });
            move || {
                if let Some(observer) = observer {
                    observer.disconnect();
                }
                drop(listener);
                drop(callback);
            }
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
                r.refit();
                redraw_report(&c, &scene, &mut r, &on_view);
            }
        })
    };
    let style = if props.fill {
        "position:relative;height:100%;min-height:500px"
    } else {
        "position:relative"
    };
    html! {<div class="vector-scene" {style} data-scene-version={props.scene.version.to_string()}><canvas ref={canvas} class="native-viewer vector-scene-canvas" style="touch-action:none;display:block" aria-label="Interactive vector scene" onpointerdown={down} onpointermove={moved} onpointerup={up} onpointercancel={cancel.clone()} onlostpointercapture={cancel} onwheel={wheel}/><button class="vector-scene-fit" style="position:absolute;right:12px;top:12px" onclick={reset}>{"Fit"}</button></div>}
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

#[cfg(test)]
mod tests {
    use super::{fit_view, frames};
    use vector_view::{view::View, BBox};

    const BOARD: BBox = BBox {
        min: [100.0, 50.0],
        max: [150.0, 80.0],
    };

    #[test]
    fn published_view_round_trips() {
        let view = View {
            scale: 12.5,
            tx: -3.25,
            ty: 480.0,
            rotation: 0.5,
            mirrored: true,
            y_up: false,
        };
        assert_eq!(super::decode_view(&super::encode_view(&view)), Some(view));
        assert_eq!(super::decode_view("junk"), None);
        assert_eq!(super::decode_view("0 0 0 0 0 0"), None);
    }

    fn screen_extent(v: &View, b: &BBox) -> (f64, f64) {
        let a = v.to_screen(b.min);
        let c = v.to_screen(b.max);
        ((c[0] - a[0]).abs(), (c[1] - a[1]).abs())
    }

    #[test]
    fn fill_fit_covers_requested_share_of_limiting_side() {
        let mut v = View::default();
        fit_view(&mut v, &BOARD, 1200.0, 600.0, Some(0.78));
        let (w, h) = screen_extent(&v, &BOARD);
        // 50 x 30 board in 1200 x 600: height limits.
        assert!((h - 0.78 * 600.0).abs() < 1e-6, "{w} x {h}");
        let centre = v.to_screen([125.0, 65.0]);
        assert!((centre[0] - 600.0).abs() < 1e-6 && (centre[1] - 300.0).abs() < 1e-6);
    }

    #[test]
    fn fit_without_fill_uses_padded_viewport() {
        let mut v = View::default();
        fit_view(&mut v, &BOARD, 1200.0, 600.0, None);
        let (_, h) = screen_extent(&v, &BOARD);
        assert!((h - (600.0 - 2.0 * super::FIT_PAD)).abs() < 1e-6);
    }

    #[test]
    fn collapsed_viewport_camera_is_not_a_usable_frame() {
        // What a fit against a 1x1 canvas produced, then persisted.
        let mut collapsed = View::default();
        fit_view(&mut collapsed, &BOARD, 1.0, 1.0, Some(0.78));
        assert!(!frames(&collapsed, &BOARD, 1242.0, 567.0));
        let mut good = View::default();
        fit_view(&mut good, &BOARD, 1242.0, 567.0, Some(0.78));
        assert!(frames(&good, &BOARD, 1242.0, 567.0));
        // Panned entirely off-screen.
        let mut away = good;
        away.pan(5000.0, 0.0);
        assert!(!frames(&away, &BOARD, 1242.0, 567.0));
        // Zoomed in past the board edges still frames it.
        let mut close = good;
        close.zoom_at(621.0, 283.0, 20.0);
        assert!(frames(&close, &BOARD, 1242.0, 567.0));
    }
}
