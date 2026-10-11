//! Annotations for an agent: a note pinned to a dragged-out area of the
//! current view, plus a snapshot (of that area, or the whole view), queued on
//! the Agent Portal edit stack of the session that opened this workbench.
//! Clicking the corner button starts the area selector at once; the drag ends
//! in the composer with the note focused, and Shift+Enter sends. Portal serves `POST /__portal/edit-stack` on the forward origin;
//! outside a Portal forward there is no queue and the composer says so.
//!
//! Spatial feedback is the point: the selected area travels as the cropped
//! image, as canvas pixels, and, for the vector views (schematic, PCB), as
//! scene millimetres read from the camera the canvas publishes, so an agent
//! can map the note onto references and nets without the image.

use gloo_events::EventListener;
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
    /// Something worth knowing that is not an error (muted).
    Hint(String),
    Sending,
    Done(String),
    Failed(String),
}

/// Whether a composer key press means "send": Shift+Enter, or the older
/// Ctrl/Cmd+Enter. Plain Enter keeps inserting a newline.
pub fn is_send_key(key: &str, shift: bool, ctrl: bool, meta: bool) -> bool {
    key == "Enter" && (shift || ctrl || meta)
}

/// Client coordinates of a drag in progress: where it started, where it is.
type Drag = Option<([f64; 2], [f64; 2])>;

fn client(e: &PointerEvent) -> [f64; 2] {
    [f64::from(e.client_x()), f64::from(e.client_y())]
}

#[function_component(Annotator)]
pub fn annotator(props: &Props) -> Html {
    let root = use_node_ref();
    let textarea = use_node_ref();
    let open = use_state(|| false);
    let note = use_state(String::new);
    let with_snapshot = use_state(|| true);
    let status = use_state(|| Status::Idle);
    let selecting = use_state(|| false);
    let drag: UseStateHandle<Drag> = use_state(|| None);
    let selection = use_state(|| None::<Selection>);

    // A selection belongs to the view it was drawn on.
    {
        let selection = selection.clone();
        let selecting = selecting.clone();
        use_effect_with(props.tab.clone(), move |_| {
            selection.set(None);
            selecting.set(false);
        });
    }
    // Escape abandons a selection in progress and closes the annotator: the
    // flow starts with the selector, so backing out of it backs out entirely.
    {
        let selecting = selecting.clone();
        let drag = drag.clone();
        let open = open.clone();
        use_effect_with(*selecting, move |active| {
            let listener = active.then(|| {
                web_sys::window().map(|w| {
                    EventListener::new(&w, "keydown", move |e| {
                        if e.dyn_ref::<KeyboardEvent>()
                            .is_some_and(|k| k.key() == "Escape")
                        {
                            drag.set(None);
                            selecting.set(false);
                            open.set(false);
                        }
                    })
                })
            });
            move || drop(listener)
        });
    }
    // The composer appears once the drag ends; put the caret in the note so
    // typing can start without another click.
    {
        let textarea = textarea.clone();
        use_effect_with((*open, *selecting), move |(open, selecting)| {
            if *open && !*selecting {
                if let Some(el) = textarea.cast::<HtmlTextAreaElement>() {
                    let _ = el.focus();
                }
            }
        });
    }

    let stage = || {
        root.cast::<Element>()
            .and_then(|el| el.closest(".view-stage").ok().flatten())
    };
    // Opening goes straight into the area selector; closing drops everything.
    let toggle = {
        let open = open.clone();
        let status = status.clone();
        let selecting = selecting.clone();
        let drag = drag.clone();
        let selection = selection.clone();
        Callback::from(move |_| {
            status.set(Status::Idle);
            drag.set(None);
            if *open {
                selecting.set(false);
                open.set(false);
            } else {
                selection.set(None);
                selecting.set(true);
                open.set(true);
            }
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
    let pointer_down = {
        let drag = drag.clone();
        Callback::from(move |e: PointerEvent| {
            if e.button() != 0 {
                return;
            }
            e.prevent_default();
            if let Some(target) = e.target().and_then(|t| t.dyn_into::<Element>().ok()) {
                let _ = target.set_pointer_capture(e.pointer_id());
            }
            let p = client(&e);
            drag.set(Some((p, p)));
        })
    };
    let pointer_move = {
        let drag = drag.clone();
        Callback::from(move |e: PointerEvent| {
            if let Some((start, _)) = *drag {
                drag.set(Some((start, client(&e))));
            }
        })
    };
    let pointer_up = {
        let drag = drag.clone();
        let selecting = selecting.clone();
        let selection = selection.clone();
        let status = status.clone();
        let root = root.clone();
        Callback::from(move |e: PointerEvent| {
            let Some((start, _)) = *drag else { return };
            let end = client(&e);
            drag.set(None);
            selecting.set(false);
            let stage = root
                .cast::<Element>()
                .and_then(|el| el.closest(".view-stage").ok().flatten());
            match stage.and_then(|s| select(&s, start, end)) {
                Some(found) => selection.set(Some(found)),
                // A click or a tiny drag still opens the composer: the note
                // then carries the whole view, and "Select area" is a click away.
                None => status.set(Status::Hint(
                    "No area pinned (the drag was too small); the note covers the whole view."
                        .into(),
                )),
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
        if is_send_key(&e.key(), e.shift_key(), e.ctrl_key(), e.meta_key()) {
            e.prevent_default();
            send.emit(());
        }
    });
    let sending = *status == Status::Sending;
    let status_line = match &*status {
        Status::Idle => html! {<span class="muted">{"Shift+Enter to send"}</span>},
        Status::Hint(m) => html! {<span class="muted">{m.clone()}</span>},
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
                        <div class="annotate-hint">{"Drag out the area to annotate · Esc cancels"}</div>
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
                <button class="annotate-mini" onclick={start_select}>{"⬚ Select area"}</button>
                <span class="muted">{"Pin the note to part of the view"}</span>
            </div>
        },
    };
    html! {
        // While an area is being dragged out the composer gets out of the way;
        // the overlay's hint stands in for it.
        <div ref={root} class={classes!("annotator", open.then_some("open"))}>
            <button class="annotate-toggle" title="Annotate this view for an agent"
                aria-expanded={open.to_string()} hidden={*selecting} onclick={toggle}>
                {if *open {"✕ Close"} else {"✎ Annotate"}}
            </button>
            {if *open && !*selecting { html! {
                <div class="annotation-composer">
                    <label>{format!("{} · {}", props.tab, props.revision)}</label>
                    <textarea ref={textarea} rows="3" placeholder="What should the agent look at or change here? Shift+Enter sends."
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
    fn shift_enter_sends_and_plain_enter_does_not() {
        assert!(is_send_key("Enter", true, false, false));
        assert!(is_send_key("Enter", false, true, false));
        assert!(is_send_key("Enter", false, false, true));
        assert!(!is_send_key("Enter", false, false, false));
        assert!(!is_send_key("a", true, false, false));
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
