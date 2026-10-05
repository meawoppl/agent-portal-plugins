//! Annotations for an agent: a note plus a snapshot of the current view,
//! queued on the Agent Portal edit stack of the session that opened this
//! workbench. Portal serves `POST /__portal/edit-stack` on the forward origin;
//! outside a Portal forward there is no queue and the composer says so.

use gloo_net::http::Request;
use serde_json::{json, Value};
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;
use web_sys::{HtmlCanvasElement, HtmlTextAreaElement, KeyboardEvent};
use yew::prelude::*;

/// Portal rejects images above 1.5M data-URL characters; stay well below.
const SNAPSHOT_MAX_EDGE: f64 = 1600.0;
const SNAPSHOT_MAX_CHARS: usize = 1_400_000;

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

/// JPEG snapshot of the largest canvas inside `stage`, scaled down to fit.
fn snapshot(stage: &web_sys::Element) -> Option<String> {
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
    let source = best?;
    let (w, h) = (f64::from(source.width()), f64::from(source.height()));
    let scale = (SNAPSHOT_MAX_EDGE / w.max(h)).min(1.0);
    let document = web_sys::window()?.document()?;
    let out: HtmlCanvasElement = document.create_element("canvas").ok()?.dyn_into().ok()?;
    out.set_width((w * scale).round().max(1.0) as u32);
    out.set_height((h * scale).round().max(1.0) as u32);
    let ctx: web_sys::CanvasRenderingContext2d = out.get_context("2d").ok()??.dyn_into().ok()?;
    // Views draw on a transparent canvas over a CSS background.
    ctx.set_fill_style_str("#11131d");
    ctx.fill_rect(0.0, 0.0, f64::from(out.width()), f64::from(out.height()));
    ctx.draw_image_with_html_canvas_element_and_dw_and_dh(
        &source,
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

#[function_component(Annotator)]
pub fn annotator(props: &Props) -> Html {
    let root = use_node_ref();
    let open = use_state(|| false);
    let note = use_state(String::new);
    let with_snapshot = use_state(|| true);
    let status = use_state(|| Status::Idle);
    let toggle = {
        let open = open.clone();
        let status = status.clone();
        Callback::from(move |_| {
            status.set(Status::Idle);
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
    let send = {
        let (note, status, root) = (note.clone(), status.clone(), root.clone());
        let with_snapshot = *with_snapshot;
        let (project, tab, revision) = (
            props.project.to_string(),
            props.tab.to_string(),
            props.revision.to_string(),
        );
        Callback::from(move |_: ()| {
            let body = note.trim().to_string();
            if body.is_empty() || *status == Status::Sending {
                return;
            }
            let image = with_snapshot
                .then(|| {
                    root.cast::<web_sys::Element>()
                        .and_then(|el| el.closest(".view-stage").ok().flatten())
                        .and_then(|stage| snapshot(&stage))
                })
                .flatten();
            let page = web_sys::window()
                .and_then(|w| w.location().href().ok())
                .unwrap_or_default();
            let source = json!({
                "plugin": "kicadmium", "project": project, "revision": revision, "page": page,
            });
            // An annotation is evidence and a request for review, never
            // permission to change the board.
            let mut item = json!({
                "title": title_from(&body, "KiCad annotation"),
                "body": body,
                "context": {
                    "plugin": "kicadmium", "project": project, "tab": tab,
                    "revision": revision, "page": page, "authorizedRepair": false,
                },
            });
            if let Some(image) = image {
                item["imageDataUrl"] = image.into();
            }
            let (note, status) = (note.clone(), status.clone());
            status.set(Status::Sending);
            spawn_local(async move {
                match enqueue(source, vec![item]).await {
                    Ok(msg) => {
                        note.set(String::new());
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
        Status::Idle => html! {<span class="muted">{"Ctrl+Enter to send"}</span>},
        Status::Sending => html! {<span class="muted">{"Sending…"}</span>},
        Status::Done(m) => html! {<span class="annotate-ok">{format!("✓ {m}")}</span>},
        Status::Failed(m) => html! {<span class="annotate-err">{m.clone()}</span>},
    };
    html! {
        <div ref={root} class={classes!("annotator", open.then_some("open"))}>
            <button class="annotate-toggle" title="Annotate this view for an agent"
                aria-expanded={open.to_string()} onclick={toggle}>
                {if *open {"✕ Close"} else {"✎ Annotate"}}
            </button>
            {if *open { html! {
                <div class="annotation-composer">
                    <label>{format!("{} · {}", props.tab, props.revision)}</label>
                    <textarea rows="3" placeholder="What should the agent look at or change in this view?"
                        value={(*note).clone()} oninput={input} onkeydown={on_key}/>
                    <label class="annotate-snap">
                        <input type="checkbox" checked={*with_snapshot} onchange={toggle_snapshot}/>
                        {" Attach a snapshot of this view"}
                    </label>
                    <div class="annotation-actions">
                        {status_line}
                        <button class="annotate-send" disabled={sending || note.trim().is_empty()}
                            onclick={on_click_send}>{"Send to agent"}</button>
                    </div>
                </div>
            }} else { Html::default() }}
        </div>
    }
}
