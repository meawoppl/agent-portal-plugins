//! Annotations for an agent: a note, optionally pinned to a dragged-out area
//! of the current view, plus a snapshot (of that area, or the whole view),
//! queued on the Agent Portal edit stack of the session that opened this
//! workbench. Portal serves `POST /__portal/edit-stack` on the forward origin;
//! outside a Portal forward there is no queue and the composer says so.
//!
//! Spatial feedback is the point: the selected area travels as the cropped
//! image, as canvas pixels, and, for the vector views (schematic, PCB), as
//! scene millimetres read from the camera the canvas publishes, so an agent
//! can map the note onto references and nets without the image.

use gloo_events::{EventListener, EventListenerOptions, EventListenerPhase};
use gloo_net::http::Request;
use serde_json::{json, Value};
use vector_view::view::View;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;
use web_sys::{Element, HtmlCanvasElement, HtmlTextAreaElement, KeyboardEvent, PointerEvent};
use yew::prelude::*;

/// Portal rejects images above 1.5M data-URL characters; stay well below.
const SNAPSHOT_MAX_EDGE: f64 = 1600.0;
const SNAPSHOT_MAX_CHARS: usize = 1_400_000;
/// Drags smaller than this (CSS pixels) are taken as an accidental click.
const MIN_REGION_PX: f64 = 4.0;

/// Queue edit-stack items. Ok carries a short success message, Err a reason
/// written for the person who clicked.
pub async fn enqueue(source: Value, items: Vec<Value>) -> Result<String, String> {
    let payload = json!({ "source": source, "items": items });
    let response = Request::post("/__portal/edit-stack")
        .header("content-type", "application/json")
        .body(payload.to_string())
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|_| "Network error: the Portal queue could not be reached".to_string())?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    match status {
        // Only Portal's own reply counts: anything else (an HTML page from a
        // server that merely answered 200) means nothing was queued.
        200..=299
            if serde_json::from_str::<Value>(&text)
                .is_ok_and(|v| v.get("items").is_some_and(Value::is_array)) =>
        {
            Ok("Queued for the agent".into())
        }
        200..=299 | 404 | 405 => Err(
            "No queue here: annotations work when this workbench is opened through Agent Portal."
                .into(),
        ),
        401 => Err("Portal sign-in for this view expired. Reopen it from Agent Portal.".into()),
        _ => {
            let text = text.trim();
            Err(if text.is_empty() || text.starts_with('<') {
                format!("Queue rejected the annotation (HTTP {status})")
            } else {
                format!("Queue rejected the annotation: {text}")
            })
        }
    }
}

/// First line of `body`, shortened to Portal's title length.
pub fn title_from(body: &str, fallback: &str) -> String {
    let line = body
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or(fallback);
    if line.chars().count() > 60 {
        format!("{}…", line.chars().take(57).collect::<String>())
    } else {
        line.to_string()
    }
}

/// A dragged-out area of the view, in CSS pixels of the source canvas.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Region {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    /// CSS size of the canvas the region was taken from.
    pub canvas_w: f64,
    pub canvas_h: f64,
}

/// Order and clamp two drag corners (canvas CSS pixels) into a region; `None`
/// for a drag too small to mean anything.
pub fn region_from_drag(a: [f64; 2], b: [f64; 2], canvas_w: f64, canvas_h: f64) -> Option<Region> {
    let clamp = |p: [f64; 2]| [p[0].clamp(0.0, canvas_w), p[1].clamp(0.0, canvas_h)];
    let (a, b) = (clamp(a), clamp(b));
    let (x0, x1) = (a[0].min(b[0]), a[0].max(b[0]));
    let (y0, y1) = (a[1].min(b[1]), a[1].max(b[1]));
    (x1 - x0 >= MIN_REGION_PX && y1 - y0 >= MIN_REGION_PX).then_some(Region {
        x: x0,
        y: y0,
        w: x1 - x0,
        h: y1 - y0,
        canvas_w,
        canvas_h,
    })
}

/// Axis-aligned scene bounds (millimetres) covering the region under `view`;
/// all four corners are mapped so rotation and mirroring are honoured.
pub fn world_bounds(view: &View, r: &Region) -> [[f64; 2]; 2] {
    let corners = [
        [r.x, r.y],
        [r.x + r.w, r.y],
        [r.x, r.y + r.h],
        [r.x + r.w, r.y + r.h],
    ]
    .map(|p| view.to_world(p));
    let mut min = corners[0];
    let mut max = corners[0];
    for p in &corners[1..] {
        min = [min[0].min(p[0]), min[1].min(p[1])];
        max = [max[0].max(p[0]), max[1].max(p[1])];
    }
    [min, max]
}

/// What the region means, for an agent reading the queued text without the
/// image: millimetre bounds for vector views, viewport fractions otherwise.
pub fn region_text(r: &Region, world: Option<[[f64; 2]; 2]>, layers: &[String]) -> String {
    let mut text = match world {
        Some([min, max]) => format!(
            "Region: x {:.2}…{:.2} mm, y {:.2}…{:.2} mm (scene frame, y down)",
            min[0], max[0], min[1], max[1]
        ),
        None => format!(
            "Region: {}–{}% across, {}–{}% down the view",
            pct(r.x / r.canvas_w),
            pct((r.x + r.w) / r.canvas_w),
            pct(r.y / r.canvas_h),
            pct((r.y + r.h) / r.canvas_h)
        ),
    };
    if !layers.is_empty() {
        text.push_str(&format!("; visible copper: {}", layers.join(", ")));
    }
    text
}

fn pct(v: f64) -> String {
    format!("{}", (v * 100.0).round() as i64)
}

/// Structured form of the region for `context.region`.
pub fn region_context(r: &Region, world: Option<[[f64; 2]; 2]>, layers: &[String]) -> Value {
    let mut value = json!({
        "canvasPx": { "x": r.x, "y": r.y, "width": r.w, "height": r.h },
        "canvasSizePx": { "width": r.canvas_w, "height": r.canvas_h },
        "normalized": {
            "x": r.x / r.canvas_w, "y": r.y / r.canvas_h,
            "width": r.w / r.canvas_w, "height": r.h / r.canvas_h,
        },
    });
    if let Some([min, max]) = world {
        value["scene"] = json!({
            "units": "mm", "yDown": true,
            "x0": min[0], "y0": min[1], "x1": max[0], "y1": max[1],
        });
    }
    if !layers.is_empty() {
        value["visibleCopperLayers"] = json!(layers);
    }
    value
}

/// The view's drawing surface: the largest canvas inside `stage`.
fn largest_canvas(stage: &Element) -> Option<HtmlCanvasElement> {
    let canvases = stage.query_selector_all("canvas").ok()?;
    let mut best: Option<HtmlCanvasElement> = None;
    for i in 0..canvases.length() {
        let Some(c) = canvases
            .item(i)
            .and_then(|n| n.dyn_into::<HtmlCanvasElement>().ok())
        else {
            continue;
        };
        let area = c.width() * c.height();
        if area > 0 && best.as_ref().is_none_or(|b| area > b.width() * b.height()) {
            best = Some(c);
        }
    }
    best
}

/// Visible copper layers a view advertises for annotation context
/// (`data-annotate-layers`, comma separated), if any.
fn advertised_layers(stage: &Element) -> Vec<String> {
    stage
        .query_selector("[data-annotate-layers]")
        .ok()
        .flatten()
        .and_then(|el| el.get_attribute("data-annotate-layers"))
        .map(|list| {
            list.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// JPEG of `source` (the whole canvas, or `region` of it), scaled to fit.
fn snapshot(source: &HtmlCanvasElement, region: Option<&Region>) -> Option<String> {
    // Backing-store pixels per CSS pixel; the region is in CSS pixels.
    let rect = source.get_bounding_client_rect();
    let (bw, bh) = (f64::from(source.width()), f64::from(source.height()));
    let (sx, sy, sw, sh) = match region {
        Some(r) => {
            let kx = bw / rect.width().max(1.0);
            let ky = bh / rect.height().max(1.0);
            let sx = (r.x * kx).clamp(0.0, bw - 1.0);
            let sy = (r.y * ky).clamp(0.0, bh - 1.0);
            (
                sx,
                sy,
                (r.w * kx).min(bw - sx).max(1.0),
                (r.h * ky).min(bh - sy).max(1.0),
            )
        }
        None => (0.0, 0.0, bw, bh),
    };
    let scale = (SNAPSHOT_MAX_EDGE / sw.max(sh)).min(1.0);
    let document = web_sys::window()?.document()?;
    let out: HtmlCanvasElement = document.create_element("canvas").ok()?.dyn_into().ok()?;
    out.set_width((sw * scale).round().max(1.0) as u32);
    out.set_height((sh * scale).round().max(1.0) as u32);
    let ctx: web_sys::CanvasRenderingContext2d = out.get_context("2d").ok()??.dyn_into().ok()?;
    // Views draw on a transparent canvas over a CSS background.
    ctx.set_fill_style_str("#11131d");
    ctx.fill_rect(0.0, 0.0, f64::from(out.width()), f64::from(out.height()));
    ctx.draw_image_with_html_canvas_element_and_sw_and_sh_and_dx_and_dy_and_dw_and_dh(
        source,
        sx,
        sy,
        sw,
        sh,
        0.0,
        0.0,
        f64::from(out.width()),
        f64::from(out.height()),
    )
    .ok()?;
    [0.85, 0.7, 0.5]
        .into_iter()
        .filter_map(|q| {
            out.to_data_url_with_type_and_encoder_options("image/jpeg", &q.into())
                .ok()
        })
        .find(|url| url.len() <= SNAPSHOT_MAX_CHARS)
}

/// A completed area selection, captured the moment the drag ended so the
/// image matches what the person saw even if they pan afterwards.
#[derive(Clone, PartialEq)]
struct Selection {
    region: Region,
    world: Option<[[f64; 2]; 2]>,
    layers: Vec<String>,
    image: Option<String>,
}

fn select(stage: &Element, start: [f64; 2], end: [f64; 2]) -> Option<Selection> {
    let canvas = largest_canvas(stage)?;
    let rect = canvas.get_bounding_client_rect();
    let local = |p: [f64; 2]| [p[0] - rect.left(), p[1] - rect.top()];
    let region = region_from_drag(local(start), local(end), rect.width(), rect.height())?;
    let world = crate::vector_scene::view_of(&canvas).map(|v| world_bounds(&v, &region));
    Some(Selection {
        region,
        world,
        layers: advertised_layers(stage),
        image: snapshot(&canvas, Some(&region)),
    })
}

#[derive(Properties, PartialEq)]
pub struct Props {
    pub project: AttrValue,
    pub tab: AttrValue,
    pub revision: AttrValue,
}

#[derive(Clone, PartialEq)]
enum Status {
    Idle,
    Sending,
    Done(String),
    Failed(String),
}

/// Client coordinates of a drag in progress: where it started, where it is.
type Drag = Option<([f64; 2], [f64; 2])>;

fn client(e: &PointerEvent) -> [f64; 2] {
    [f64::from(e.client_x()), f64::from(e.client_y())]
}

/// Whether the device is driven by touch first: a phone, any coarse primary
/// pointer, or an iPad, which Safari reports as a Mac so only its touch points
/// give it away. Touch devices get the bottom-sheet composer and big buttons;
/// desktops keep the floating composer.
pub fn touch_primary_from(coarse: bool, max_touch_points: i32, user_agent: &str) -> bool {
    coarse || (max_touch_points > 1 && user_agent.contains("Macintosh"))
}

fn touch_primary() -> bool {
    let Some(window) = web_sys::window() else {
        return false;
    };
    let coarse = window
        .match_media("(pointer: coarse)")
        .ok()
        .flatten()
        .is_some_and(|m| m.matches());
    let navigator = window.navigator();
    touch_primary_from(
        coarse,
        navigator.max_touch_points(),
        &navigator.user_agent().unwrap_or_default(),
    )
}

/// Whether a pointerdown on the view starts drawing an area straight away.
/// Only a pen does, so fingers keep panning and pinching the view; never while
/// the overlay is already collecting a drag, and never on the composer.
pub fn pen_starts_area(
    pointer_type: &str,
    button: i16,
    selecting: bool,
    on_composer: bool,
) -> bool {
    pointer_type == "pen" && button == 0 && !selecting && !on_composer
}

/// CSS pixels of the layout viewport hidden under the on-screen keyboard. iOS
/// overlays the keyboard instead of resizing the page, so a bottom-docked
/// sheet lifts itself by this much to stay above it.
pub fn keyboard_inset(inner_height: f64, viewport_height: f64, viewport_offset_top: f64) -> f64 {
    (inner_height - viewport_height - viewport_offset_top).max(0.0)
}

/// The view-stage element an annotator belongs to.
fn stage_of(root: &NodeRef) -> Option<Element> {
    root.cast::<Element>()
        .and_then(|el| el.closest(".view-stage").ok().flatten())
}

#[function_component(Annotator)]
pub fn annotator(props: &Props) -> Html {
    let root = use_node_ref();
    let open = use_state(|| false);
    let note = use_state(String::new);
    let with_snapshot = use_state(|| true);
    let status = use_state(|| Status::Idle);
    let selecting = use_state(|| false);
    let drag: UseStateHandle<Drag> = use_state(|| None);
    let selection = use_state(|| None::<Selection>);
    let touch = *use_memo((), |_| touch_primary());
    let keyboard = use_state(|| 0.0_f64);
    // Listeners registered once per view read these instead of a state
    // handle, which would only ever show them the render they were made in.
    let selecting_now = use_mut_ref(|| false);
    *selecting_now.borrow_mut() = *selecting;
    // The pointer drawing the area (a pen on the view, or the first finger on
    // the overlay) and, for the pen, where it touched down.
    let pen = use_mut_ref(|| None::<(i32, [f64; 2])>);
    let finger = use_mut_ref(|| None::<i32>);

    // A selection belongs to the view it was drawn on.
    {
        let selection = selection.clone();
        let selecting = selecting.clone();
        use_effect_with(props.tab.clone(), move |_| {
            selection.set(None);
            selecting.set(false);
        });
    }
    // Escape abandons a selection in progress.
    {
        let selecting = selecting.clone();
        let drag = drag.clone();
        use_effect_with(*selecting, move |active| {
            let listener = active.then(|| {
                web_sys::window().map(|w| {
                    EventListener::new(&w, "keydown", move |e| {
                        if e.dyn_ref::<KeyboardEvent>()
                            .is_some_and(|k| k.key() == "Escape")
                        {
                            drag.set(None);
                            selecting.set(false);
                        }
                    })
                })
            });
            move || drop(listener)
        });
    }
    // A pen draws an area straight onto the view, no mode to enter first.
    // Capture-phase listeners on the stage see the pen before the canvas does
    // and stop it there, so neither the vector view nor the 3D orbit controls
    // start a pan; fingers pass through untouched and still navigate.
    {
        let root = root.clone();
        let (drag, selecting, selection) = (drag.clone(), selecting.clone(), selection.clone());
        let (open, status) = (open.clone(), status.clone());
        let (pen, selecting_now) = (pen.clone(), selecting_now.clone());
        use_effect_with(props.tab.clone(), move |_| {
            let listeners = stage_of(&root).map(|stage| {
                let capture = EventListenerOptions {
                    phase: EventListenerPhase::Capture,
                    passive: false,
                };
                let down = {
                    let target = stage.clone();
                    let (pen, drag, selecting, status) =
                        (pen.clone(), drag.clone(), selecting.clone(), status.clone());
                    EventListener::new_with_options(&stage, "pointerdown", capture, move |e| {
                        let Some(e) = e.dyn_ref::<PointerEvent>() else {
                            return;
                        };
                        let on_composer = e
                            .target()
                            .and_then(|t| t.dyn_into::<Element>().ok())
                            .and_then(|t| t.closest(".annotator").ok().flatten())
                            .is_some();
                        if !pen_starts_area(
                            &e.pointer_type(),
                            e.button(),
                            *selecting_now.borrow(),
                            on_composer,
                        ) {
                            return;
                        }
                        e.prevent_default();
                        e.stop_propagation();
                        let _ = target.set_pointer_capture(e.pointer_id());
                        let p = client(e);
                        *pen.borrow_mut() = Some((e.pointer_id(), p));
                        status.set(Status::Idle);
                        drag.set(Some((p, p)));
                        selecting.set(true);
                    })
                };
                let moved = {
                    let (pen, drag) = (pen.clone(), drag.clone());
                    EventListener::new_with_options(&stage, "pointermove", capture, move |e| {
                        let Some(e) = e.dyn_ref::<PointerEvent>() else {
                            return;
                        };
                        let Some((id, start)) = *pen.borrow() else {
                            return;
                        };
                        if e.pointer_id() == id {
                            e.stop_propagation();
                            drag.set(Some((start, client(e))));
                        }
                    })
                };
                let finish = |kind: &'static str| {
                    let stage = stage.clone();
                    let (pen, drag, selecting) = (pen.clone(), drag.clone(), selecting.clone());
                    let (selection, open) = (selection.clone(), open.clone());
                    EventListener::new_with_options(&stage.clone(), kind, capture, move |e| {
                        let Some(e) = e.dyn_ref::<PointerEvent>() else {
                            return;
                        };
                        let Some((id, start)) = *pen.borrow() else {
                            return;
                        };
                        if e.pointer_id() != id {
                            return;
                        }
                        e.stop_propagation();
                        pen.borrow_mut().take();
                        drag.set(None);
                        selecting.set(false);
                        // A pen tap marks nothing; a stroke opens the composer
                        // on the area it drew.
                        if kind == "pointerup" {
                            if let Some(found) = select(&stage, start, client(e)) {
                                selection.set(Some(found));
                                open.set(true);
                            }
                        }
                    })
                };
                [down, moved, finish("pointerup"), finish("pointercancel")]
            });
            move || drop(listeners)
        });
    }
    // iOS slides the keyboard over the page; track how much it hides so the
    // bottom sheet can sit on top of it.
    {
        let keyboard = keyboard.clone();
        use_effect_with(touch && *open, move |active| {
            let listeners = active
                .then(web_sys::window)
                .flatten()
                .and_then(|w| w.visual_viewport().map(|vv| (w, vv)))
                .map(|(window, vv)| {
                    let measure = {
                        let (window, vv, keyboard) = (window.clone(), vv.clone(), keyboard.clone());
                        move || {
                            let inner = window
                                .inner_height()
                                .ok()
                                .and_then(|h| h.as_f64())
                                .unwrap_or(0.0);
                            keyboard.set(keyboard_inset(inner, vv.height(), vv.offset_top()));
                        }
                    };
                    measure();
                    let on_resize = {
                        let measure = measure.clone();
                        EventListener::new(&vv, "resize", move |_| measure())
                    };
                    let on_scroll = EventListener::new(&vv, "scroll", move |_| measure());
                    [on_resize, on_scroll]
                });
            if !*active {
                keyboard.set(0.0);
            }
            move || drop(listeners)
        });
    }

    let stage = || stage_of(&root);
    let toggle = {
        let open = open.clone();
        let status = status.clone();
        let selecting = selecting.clone();
        Callback::from(move |_| {
            status.set(Status::Idle);
            selecting.set(false);
            open.set(!*open)
        })
    };
    let input = {
        let note = note.clone();
        Callback::from(move |e: InputEvent| {
            note.set(e.target_unchecked_into::<HtmlTextAreaElement>().value())
        })
    };
    let toggle_snapshot = {
        let with_snapshot = with_snapshot.clone();
        Callback::from(move |_| with_snapshot.set(!*with_snapshot))
    };
    let start_select = {
        let selecting = selecting.clone();
        let drag = drag.clone();
        let status = status.clone();
        Callback::from(move |_: MouseEvent| {
            status.set(Status::Idle);
            drag.set(None);
            selecting.set(true);
        })
    };
    let clear_selection = {
        let selection = selection.clone();
        Callback::from(move |_: MouseEvent| selection.set(None))
    };
    let cancel_select = {
        let (drag, selecting, finger) = (drag.clone(), selecting.clone(), finger.clone());
        Callback::from(move |_: MouseEvent| {
            finger.borrow_mut().take();
            drag.set(None);
            selecting.set(false);
        })
    };
    let pointer_down = {
        let (drag, selecting, finger) = (drag.clone(), selecting.clone(), finger.clone());
        Callback::from(move |e: PointerEvent| {
            if e.button() != 0 {
                return;
            }
            let on_cancel = e
                .target()
                .and_then(|t| t.dyn_into::<Element>().ok())
                .and_then(|t| t.closest(".annotate-cancel").ok().flatten())
                .is_some();
            if on_cancel {
                return;
            }
            e.prevent_default();
            // A second finger means a pinch, not a box: give the view back.
            let active = *finger.borrow();
            if active.is_some_and(|id| id != e.pointer_id()) {
                finger.borrow_mut().take();
                drag.set(None);
                selecting.set(false);
                return;
            }
            if let Some(target) = e.target().and_then(|t| t.dyn_into::<Element>().ok()) {
                let _ = target.set_pointer_capture(e.pointer_id());
            }
            *finger.borrow_mut() = Some(e.pointer_id());
            let p = client(&e);
            drag.set(Some((p, p)));
        })
    };
    let pointer_move = {
        let (drag, finger) = (drag.clone(), finger.clone());
        Callback::from(move |e: PointerEvent| {
            if *finger.borrow() != Some(e.pointer_id()) {
                return;
            }
            if let Some((start, _)) = *drag {
                drag.set(Some((start, client(&e))));
            }
        })
    };
    let pointer_up = {
        let drag = drag.clone();
        let selecting = selecting.clone();
        let selection = selection.clone();
        let (open, status, finger) = (open.clone(), status.clone(), finger.clone());
        let root = root.clone();
        Callback::from(move |e: PointerEvent| {
            if *finger.borrow() != Some(e.pointer_id()) {
                return;
            }
            finger.borrow_mut().take();
            let Some((start, _)) = *drag else { return };
            let end = client(&e);
            drag.set(None);
            selecting.set(false);
            match stage_of(&root).and_then(|s| select(&s, start, end)) {
                Some(found) => {
                    selection.set(Some(found));
                    open.set(true);
                }
                None => status.set(Status::Failed(if touch {
                    "Drag out a larger area to select it.".into()
                } else {
                    "Drag out a larger area to select it (Escape cancels).".into()
                })),
            }
        })
    };
    let send = {
        let (note, status, root) = (note.clone(), status.clone(), root.clone());
        let selection = selection.clone();
        let with_snapshot = *with_snapshot;
        let (project, tab, revision) = (
            props.project.to_string(),
            props.tab.to_string(),
            props.revision.to_string(),
        );
        Callback::from(move |_: ()| {
            let typed = note.trim().to_string();
            if typed.is_empty() || *status == Status::Sending {
                return;
            }
            let picked = (*selection).clone();
            let image = match (&picked, with_snapshot) {
                (_, false) => None,
                (Some(sel), true) => sel.image.clone(),
                (None, true) => root
                    .cast::<Element>()
                    .and_then(|el| el.closest(".view-stage").ok().flatten())
                    .and_then(|stage| largest_canvas(&stage))
                    .and_then(|c| snapshot(&c, None)),
            };
            let page = web_sys::window()
                .and_then(|w| w.location().href().ok())
                .unwrap_or_default();
            let source = json!({
                "plugin": "kicadmium", "project": project, "revision": revision, "page": page,
            });
            // The region is spelled out in the text too: an agent reading the
            // queue without the image still learns where on the board to look.
            let body = match &picked {
                Some(sel) => format!(
                    "{typed}\n\n{}",
                    region_text(&sel.region, sel.world, &sel.layers)
                ),
                None => typed.clone(),
            };
            // An annotation is evidence and a request for review, never
            // permission to change the board.
            let mut item = json!({
                "title": title_from(&typed, "KiCad annotation"),
                "body": body,
                "context": {
                    "plugin": "kicadmium", "project": project, "tab": tab,
                    "revision": revision, "page": page, "authorizedRepair": false,
                },
            });
            if let Some(sel) = &picked {
                item["context"]["region"] = region_context(&sel.region, sel.world, &sel.layers);
            }
            if let Some(image) = image {
                item["imageDataUrl"] = image.into();
                item["context"]["imageScope"] =
                    if picked.is_some() { "region" } else { "view" }.into();
            }
            let (note, status, selection) = (note.clone(), status.clone(), selection.clone());
            status.set(Status::Sending);
            spawn_local(async move {
                match enqueue(source, vec![item]).await {
                    Ok(msg) => {
                        note.set(String::new());
                        selection.set(None);
                        status.set(Status::Done(msg));
                    }
                    Err(msg) => status.set(Status::Failed(msg)),
                }
            });
        })
    };
    let on_click_send = {
        let send = send.clone();
        Callback::from(move |_| send.emit(()))
    };
    let on_key = Callback::from(move |e: KeyboardEvent| {
        if e.key() == "Enter" && (e.ctrl_key() || e.meta_key()) {
            e.prevent_default();
            send.emit(());
        }
    });
    let sending = *status == Status::Sending;
    let status_line = match &*status {
        Status::Idle if touch => {
            html! {<span class="muted">{"Draw on the view with Apple Pencil to mark an area"}</span>}
        }
        Status::Idle => html! {<span class="muted">{"Ctrl+Enter to send"}</span>},
        Status::Sending => html! {<span class="muted">{"Sending…"}</span>},
        Status::Done(m) => html! {<span class="annotate-ok">{format!("✓ {m}")}</span>},
        Status::Failed(m) => html! {<span class="annotate-err">{m.clone()}</span>},
    };
    // The drag overlay covers the whole stage, so it is rendered into the
    // stage element rather than inside this centred, transformed box.
    let overlay = match (*selecting, stage()) {
        (true, Some(stage_el)) => {
            let stage_rect = stage_el.get_bounding_client_rect();
            let marker = (*drag).map(|(a, b)| {
                let (x0, x1) = (a[0].min(b[0]) - stage_rect.left(), a[0].max(b[0]) - stage_rect.left());
                let (y0, y1) = (a[1].min(b[1]) - stage_rect.top(), a[1].max(b[1]) - stage_rect.top());
                html! {<div class="annotate-box" style={format!("left:{x0}px;top:{y0}px;width:{}px;height:{}px", x1 - x0, y1 - y0)}/>}
            });
            create_portal(
                html! {
                    <div class="annotate-overlay" role="application" aria-label="Select an area to annotate"
                        onpointerdown={pointer_down} onpointermove={pointer_move}
                        onpointerup={pointer_up.clone()} onpointercancel={pointer_up}>
                        {marker.unwrap_or_default()}
                        <div class="annotate-hint">{if touch {"Drag a box over the area · two fingers cancel"} else {"Drag to select the area · Esc cancels"}}</div>
                        {if touch && (*pen).borrow().is_none() { html! {
                            <button class="annotate-cancel" onclick={cancel_select}>{"Cancel"}</button>
                        }} else { Html::default() }}
                    </div>
                },
                stage_el,
            )
        }
        _ => Html::default(),
    };
    let region_block = match &*selection {
        Some(sel) => html! {
            <div class="annotate-region">
                {sel.image.as_ref().map(|src| html!{<img class="annotate-thumb" src={src.clone()} alt="Selected area"/>}).unwrap_or_default()}
                <span class="muted">{region_text(&sel.region, sel.world, &sel.layers)}</span>
                <div class="annotate-region-row">
                    <button class="annotate-mini" onclick={start_select.clone()}>{"Reselect area"}</button>
                    <button class="annotate-mini" onclick={clear_selection}>{"Clear area"}</button>
                </div>
            </div>
        },
        None => html! {
            <div class="annotate-region-row">
                <button class="annotate-mini" onclick={start_select.clone()}>{"⬚ Select area"}</button>
                <span class="muted">{"Pin the note to part of the view"}</span>
            </div>
        },
    };
    // On touch devices marking an area is one tap away at all times, beside
    // the toggle; with a pen it needs no tap at all.
    let mark_button = (touch && !*open && !*selecting).then(|| {
        html! {<button class="annotate-mark" title="Mark an area of this view for an agent"
        onclick={start_select}>{"⬚ Mark area"}</button>}
    });
    html! {
        // While an area is being dragged out the composer gets out of the way;
        // the overlay's hint stands in for it.
        <div ref={root} class={classes!("annotator", open.then_some("open"), touch.then_some("touch"))}>
            {mark_button.unwrap_or_default()}
            <button class="annotate-toggle" title="Annotate this view for an agent"
                aria-expanded={open.to_string()} hidden={*selecting || (touch && *open)} onclick={toggle.clone()}>
                {if *open {"✕ Close"} else {"✎ Annotate"}}
            </button>
            {if *open && !*selecting { html! {
                <div class={classes!("annotation-composer", touch.then_some("sheet"))}
                    style={touch.then(|| format!("--kb:{}px", *keyboard))}>
                    <div class="annotate-head">
                        <label>{format!("{} · {}", props.tab, props.revision)}</label>
                        {if touch { html! {
                            <button class="annotate-close" aria-label="Close" onclick={toggle}>{"✕"}</button>
                        }} else { Html::default() }}
                    </div>
                    <textarea rows="3" placeholder="What should the agent look at or change in this view?"
                        value={(*note).clone()} oninput={input} onkeydown={on_key}/>
                    {region_block}
                    <label class="annotate-snap">
                        <input type="checkbox" checked={*with_snapshot} onchange={toggle_snapshot}/>
                        {if selection.is_some() {" Attach the selected area as an image"} else {" Attach a snapshot of this view"}}
                    </label>
                    <div class="annotation-actions">
                        {status_line}
                        <button class="annotate-send" disabled={sending || note.trim().is_empty()}
                            onclick={on_click_send}>{"Send to agent"}</button>
                    </div>
                </div>
            }} else { Html::default() }}
            {overlay}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipads_count_as_touch_even_when_safari_claims_a_mac() {
        let ipad = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15";
        assert!(touch_primary_from(false, 5, ipad));
        assert!(
            !touch_primary_from(false, 0, ipad),
            "a real Mac has no touch points"
        );
        assert!(touch_primary_from(
            true,
            0,
            "Mozilla/5.0 (Linux; Android 14)"
        ));
        let windows = "Mozilla/5.0 (Windows NT 10.0; Win64; x64)";
        assert!(
            !touch_primary_from(false, 10, windows),
            "a touchscreen laptop with a fine primary pointer stays desktop"
        );
    }

    #[test]
    fn only_a_pen_on_the_view_draws_an_area() {
        assert!(pen_starts_area("pen", 0, false, false));
        assert!(
            !pen_starts_area("touch", 0, false, false),
            "fingers navigate"
        );
        assert!(!pen_starts_area("mouse", 0, false, false));
        assert!(
            !pen_starts_area("pen", 2, false, false),
            "barrel button is not a stroke"
        );
        assert!(
            !pen_starts_area("pen", 0, true, false),
            "the overlay owns an active drag"
        );
        assert!(
            !pen_starts_area("pen", 0, false, true),
            "writing in the composer"
        );
    }

    #[test]
    fn keyboard_inset_is_the_hidden_bottom_of_the_page() {
        assert_eq!(keyboard_inset(1024.0, 1024.0, 0.0), 0.0);
        assert_eq!(keyboard_inset(1024.0, 640.0, 0.0), 384.0);
        assert_eq!(
            keyboard_inset(1024.0, 640.0, 100.0),
            284.0,
            "page scrolled under it"
        );
        assert_eq!(keyboard_inset(1000.0, 1024.0, 0.0), 0.0, "never negative");
    }

    #[test]
    fn drags_are_ordered_clamped_and_debounced() {
        let r = region_from_drag([120.0, 80.0], [20.0, 30.0], 800.0, 600.0).unwrap();
        assert_eq!((r.x, r.y, r.w, r.h), (20.0, 30.0, 100.0, 50.0));
        let r = region_from_drag([-50.0, -10.0], [900.0, 650.0], 800.0, 600.0).unwrap();
        assert_eq!((r.x, r.y, r.w, r.h), (0.0, 0.0, 800.0, 600.0));
        assert!(region_from_drag([10.0, 10.0], [12.0, 40.0], 800.0, 600.0).is_none());
    }

    #[test]
    fn world_bounds_follow_the_camera() {
        // 10 px per mm, origin drawn at (100, 50); a mirrored board flips x.
        let view = View {
            scale: 10.0,
            tx: 100.0,
            ty: 50.0,
            rotation: 0.0,
            mirrored: false,
            y_up: false,
        };
        let r = region_from_drag([100.0, 50.0], [200.0, 100.0], 800.0, 600.0).unwrap();
        let [min, max] = world_bounds(&view, &r);
        assert!((min[0]).abs() < 1e-9 && (max[0] - 10.0).abs() < 1e-9);
        assert!((min[1]).abs() < 1e-9 && (max[1] - 5.0).abs() < 1e-9);
        let mirrored = View {
            mirrored: true,
            ..view
        };
        let [min, max] = world_bounds(&mirrored, &r);
        assert!(min[0] < max[0], "bounds stay ordered under mirroring");
        assert!((max[0] - min[0] - 10.0).abs() < 1e-9);
    }

    #[test]
    fn region_text_and_context_describe_the_area() {
        let r = region_from_drag([0.0, 0.0], [200.0, 300.0], 800.0, 600.0).unwrap();
        let text = region_text(&r, None, &[]);
        assert_eq!(text, "Region: 0–25% across, 0–50% down the view");
        let text = region_text(&r, Some([[1.0, 2.0], [31.5, 42.25]]), &["F.Cu".into()]);
        assert_eq!(
            text,
            "Region: x 1.00…31.50 mm, y 2.00…42.25 mm (scene frame, y down); visible copper: F.Cu"
        );
        let ctx = region_context(&r, Some([[1.0, 2.0], [31.5, 42.25]]), &["F.Cu".into()]);
        assert_eq!(ctx["normalized"]["width"], 0.25);
        assert_eq!(ctx["scene"]["units"], "mm");
        assert_eq!(ctx["scene"]["x1"], 31.5);
        assert_eq!(ctx["visibleCopperLayers"][0], "F.Cu");
        assert!(region_context(&r, None, &[]).get("scene").is_none());
    }
}
