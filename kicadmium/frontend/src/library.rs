//! Libraries tab: one card per unique (symbol, footprint, 3D model set) with a
//! symbol | footprint | 3D render triptych, badges, and part metadata. Renders
//! are produced lazily server-side; pending thumbs are polled until ready.
//! Clicking a render opens the interactive viewer in a modal.

use std::collections::BTreeMap;
use std::rc::Rc;

use gloo_events::EventListener;
use gloo_timers::callback::Timeout;
use shared::library::{LibraryPart, LibraryResponse, ThumbRef, ThumbStatus};
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;
use web_sys::{HtmlInputElement, HtmlSelectElement, KeyboardEvent};
use yew::prelude::*;

use crate::api;
use crate::model_view::ModelView;
use crate::vector_scene::VectorSceneCanvas;

const POLL_MS: u32 = 900;
const KINDS: [(&str, &str); 3] = [
    ("symbol", "Symbol"),
    ("footprint", "Footprint"),
    ("model", "3D"),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Filter {
    All,
    Used,
    Unused,
    Issues,
}

impl Filter {
    fn parse(value: &str) -> Self {
        match value {
            "used" => Self::Used,
            "unused" => Self::Unused,
            "issues" => Self::Issues,
            _ => Self::All,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Used,
    Power,
    Unused,
}

fn thumb<'a>(part: &'a LibraryPart, kind: &str) -> &'a ThumbRef {
    match kind {
        "symbol" => &part.thumbs.symbol,
        "footprint" => &part.thumbs.footprint,
        _ => &part.thumbs.model,
    }
}

/// Thumb with any polled status merged over the inventory snapshot.
fn effective(thumb: &ThumbRef, states: &BTreeMap<String, ThumbStatus>) -> ThumbRef {
    let mut merged = thumb.clone();
    if let Some(status) = thumb.key.as_ref().and_then(|key| states.get(key)) {
        merged.state = status.state.clone();
        if status.url.is_some() {
            merged.url = status.url.clone();
        }
        if status.message.is_some() {
            merged.message = status.message.clone();
        }
    }
    merged
}

fn is_pending(state: &str) -> bool {
    state == "queued" || state == "rendering"
}

fn placeholder(thumb: &ThumbRef) -> String {
    match thumb.state.as_str() {
        "queued" => "queued".into(),
        "rendering" => "rendering...".into(),
        "failed" => "render failed".into(),
        "idle" => "waiting".into(),
        _ => thumb.message.clone().unwrap_or_else(|| "n/a".into()),
    }
}

fn compact_refs(refs: &[String]) -> String {
    let shown = refs.iter().take(10).cloned().collect::<Vec<_>>().join(", ");
    if refs.len() > 10 {
        format!("{shown} +{}", refs.len() - 10)
    } else {
        shown
    }
}

fn lcsc_href(code: &str) -> String {
    let clean = code.trim();
    let is_part = clean.len() > 1
        && clean[..1].eq_ignore_ascii_case("c")
        && clean[1..].bytes().all(|b| b.is_ascii_digit());
    if is_part {
        format!(
            "https://www.lcsc.com/product-detail/{}.html",
            api::encode(&clean.to_uppercase())
        )
    } else {
        format!("https://www.lcsc.com/search?q={}", api::encode(clean))
    }
}

fn search_text(part: &LibraryPart) -> String {
    let mut fields: Vec<&str> = vec![&part.title];
    fields.extend(part.symbol.as_deref());
    fields.extend(part.footprint.as_deref());
    fields.extend(part.refs.iter().map(String::as_str));
    fields.extend(part.values.iter().map(String::as_str));
    fields.extend(part.lcsc.as_deref());
    fields.extend(part.mpn.as_deref());
    fields.extend(part.manufacturer.as_deref());
    fields.extend(part.description.as_deref());
    fields.extend(part.badges.iter().map(|b| b.label.as_str()));
    fields.join(" ").to_lowercase()
}

fn has_issue(part: &LibraryPart) -> bool {
    part.badges
        .iter()
        .any(|b| b.level == "error" || b.level == "warning")
}

fn is_power(part: &LibraryPart) -> bool {
    part.badges.iter().any(|b| b.kind == "power")
}

fn matches(part: &LibraryPart, section: Section, terms: &[String], filter: Filter) -> bool {
    let text = search_text(part);
    terms.iter().all(|term| text.contains(term))
        && match filter {
            Filter::All => true,
            Filter::Used => section != Section::Unused,
            Filter::Unused => section == Section::Unused,
            Filter::Issues => has_issue(part),
        }
}

#[derive(Properties, PartialEq)]
pub struct LibraryTabProps {
    pub project: AttrValue,
}

#[function_component(LibraryTab)]
pub fn library_tab(props: &LibraryTabProps) -> Html {
    let data = use_state(|| None::<Rc<LibraryResponse>>);
    let error = use_state(|| None::<String>);
    let states = use_state(BTreeMap::<String, ThumbStatus>::new);
    let query = use_state(String::new);
    let filter = use_state(|| Filter::All);
    let modal = use_state(|| None::<(Rc<LibraryPart>, String)>);
    let links = use_state(|| None::<String>);
    let poll_round = use_state(|| 0u32);

    {
        let data = data.clone();
        let error = error.clone();
        let states = states.clone();
        use_effect_with(props.project.clone(), move |project| {
            let project = project.to_string();
            spawn_local(async move {
                match api::library(&project).await {
                    Ok(next) => {
                        states.set(BTreeMap::new());
                        data.set(Some(Rc::new(next)));
                        error.set(None);
                    }
                    Err(err) => error.set(Some(err)),
                }
            });
        });
    }

    let state_map: &BTreeMap<String, ThumbStatus> = &states;
    let terms: Vec<String> = query
        .to_lowercase()
        .split_whitespace()
        .map(str::to_owned)
        .collect();

    // Keys still rendering, in display order, with filtered-in cards first so
    // the server renders what the user can see before the rest.
    let pending: Vec<(String, bool)> = data
        .as_ref()
        .map(|data| {
            let sections = data
                .parts
                .iter()
                .map(|p| {
                    (
                        p,
                        if is_power(p) {
                            Section::Power
                        } else {
                            Section::Used
                        },
                    )
                })
                .chain(data.unused.iter().map(|p| (p, Section::Unused)));
            sections
                .flat_map(|(part, section)| {
                    let shown = matches(part, section, &terms, *filter);
                    KINDS.iter().filter_map(move |(kind, _)| {
                        let t = effective(thumb(part, kind), state_map);
                        if is_pending(&t.state) {
                            Some((t.key.clone()?, shown))
                        } else {
                            None
                        }
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    {
        let states = states.clone();
        let poll_round = poll_round.clone();
        let project = props.project.to_string();
        let pending = pending.clone();
        use_effect_with((pending.len(), *poll_round), move |_| {
            let timeout = (!pending.is_empty()).then(|| {
                Timeout::new(if *poll_round == 0 { 100 } else { POLL_MS }, move || {
                    let mut keys: Vec<String> = pending.iter().map(|(k, _)| k.clone()).collect();
                    keys.dedup();
                    keys.truncate(400);
                    let visible: Vec<String> = pending
                        .iter()
                        .filter(|(_, shown)| *shown)
                        .map(|(k, _)| k.clone())
                        .take(60)
                        .collect();
                    spawn_local(async move {
                        if let Ok(response) = api::library_status(&project, &keys, &visible).await {
                            let mut next = (*states).clone();
                            next.extend(response.states);
                            states.set(next);
                        }
                        poll_round.set(*poll_round + 1);
                    });
                })
            });
            move || drop(timeout)
        });
    }

    {
        let modal = modal.clone();
        use_effect_with((), move |_| {
            let listener = EventListener::new(&gloo_utils::document(), "keydown", move |event| {
                if event
                    .dyn_ref::<KeyboardEvent>()
                    .is_some_and(|e| e.key() == "Escape")
                {
                    modal.set(None);
                }
            });
            move || drop(listener)
        });
    }

    if let Some(err) = &*error {
        return html! { <p class="warning">{format!("Unable to load library inventory: {err}")}</p> };
    }
    let Some(data) = (*data).clone() else {
        return html! { <p class="muted">{"Building part inventory..."}</p> };
    };

    let open = |part: &LibraryPart, kind: &str| {
        let modal = modal.clone();
        let part = Rc::new(part.clone());
        let kind = kind.to_string();
        Callback::from(move |_: MouseEvent| modal.set(Some((part.clone(), kind.clone()))))
    };

    let card = |part: &LibraryPart, section: Section| -> Option<Html> {
        if !matches(part, section, &terms, *filter) {
            return None;
        }
        let cells = KINDS.iter().map(|(kind, label)| {
            let t = effective(thumb(part, kind), state_map);
            let inner = match (&t.url, t.state.as_str()) {
                (Some(url), "ready") => {
                    html! { <img loading="lazy" alt={*label} src={url.clone()} /> }
                }
                _ => html! { <span class="ph">{placeholder(&t)}</span> },
            };
            html! {
                <button class="lib-cell" data-state={t.state.clone()} title={t.message.clone()}
                    disabled={t.key.is_none()} onclick={open(part, kind)}>
                    {inner}<span class="lab">{*label}</span>
                </button>
            }
        });
        let sub = part.footprint.as_ref().and(part.symbol.clone());
        let mut meta = Vec::new();
        if !part.refs.is_empty() {
            meta.push(html! { <strong>{compact_refs(&part.refs)}</strong> });
        }
        if !part.values.is_empty() {
            meta.push(html! { {part.values.join(", ")} });
        }
        let mut link_items = Vec::new();
        if let Some(code) = &part.lcsc {
            link_items.push(html! { <a href={lcsc_href(code)} target="_blank" rel="noopener">{format!("LCSC {}", code.trim())}</a> });
        }
        if let Some(mpn) = &part.mpn {
            let label = [part.manufacturer.as_deref(), Some(mpn.as_str())]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" ");
            link_items.push(html! { <span>{label}</span> });
        }
        if let Some(sheet) = part
            .datasheet
            .as_ref()
            .filter(|d| d.starts_with("http://") || d.starts_with("https://"))
        {
            link_items.push(
                html! { <a href={sheet.clone()} target="_blank" rel="noopener">{"datasheet"}</a> },
            );
        }
        let badges = part.badges.iter().map(|b| {
            html! { <span class={classes!("lib-badge", b.level.clone())} title={b.detail.clone()}>{&b.label}</span> }
        });
        let meta_len = meta.len();
        let meta = meta.into_iter().enumerate().map(|(i, item)| {
            html! { <>{item}{if i + 1 < meta_len { " · " } else { "" }}</> }
        });
        Some(html! {
            <article class="lib-card" key={part.id.clone()}>
                <div class="lib-trip">{for cells}</div>
                <div class="lib-body">
                    <div class="lib-title">{&part.title}</div>
                    if let Some(sub) = sub { <div class="lib-sub">{sub}</div> }
                    if meta_len > 0 { <div class="lib-meta">{for meta}</div> }
                    if let Some(desc) = &part.description { <div class="lib-meta lib-desc" title={desc.clone()}>{desc}</div> }
                    if !link_items.is_empty() { <div class="lib-links">{for link_items}</div> }
                    if !part.badges.is_empty() { <div class="lib-badges">{for badges}</div> }
                </div>
            </article>
        })
    };

    let physical: Vec<Html> = data
        .parts
        .iter()
        .filter(|p| !is_power(p))
        .filter_map(|p| card(p, Section::Used))
        .collect();
    let power: Vec<Html> = data
        .parts
        .iter()
        .filter(|p| is_power(p))
        .filter_map(|p| card(p, Section::Power))
        .collect();
    let unused: Vec<Html> = data
        .unused
        .iter()
        .filter_map(|p| card(p, Section::Unused))
        .collect();

    let ready = data
        .parts
        .iter()
        .chain(&data.unused)
        .flat_map(|p| {
            KINDS
                .iter()
                .map(move |(k, _)| effective(thumb(p, k), state_map))
        })
        .filter(|t| t.state == "ready")
        .count();
    let rendering = pending.len();
    let stats = format!(
        "{} parts · {} unused library items · {ready}/{} renders ready{} · KiCad {}",
        data.stats.parts,
        data.stats.unused,
        data.stats.thumbs,
        if rendering > 0 {
            format!(" · {rendering} rendering")
        } else {
            String::new()
        },
        data.kicad_version.as_deref().unwrap_or("?")
    );
    let libs = data
        .libraries
        .iter()
        .map(|lib| html! { <><code>{&lib.nickname}</code>{format!(" {} ({}) ", lib.kind, lib.items)}</> });

    let on_query = {
        let query = query.clone();
        Callback::from(move |e: InputEvent| {
            if let Some(input) = e.target_dyn_into::<HtmlInputElement>() {
                query.set(input.value());
            }
        })
    };
    let on_filter = {
        let filter = filter.clone();
        Callback::from(move |e: Event| {
            if let Some(select) = e.target_dyn_into::<HtmlSelectElement>() {
                filter.set(Filter::parse(&select.value()));
            }
        })
    };
    let on_links = {
        let links = links.clone();
        let project = props.project.to_string();
        Callback::from(move |_: Event| {
            if links.is_some() {
                return;
            }
            let links = links.clone();
            let project = project.clone();
            links.set(Some("Loading...".into()));
            spawn_local(async move {
                links.set(Some(match api::library_links_html(&project).await {
                    Ok(html) => html,
                    Err(err) => format!("Unable to load: {err}"),
                }));
            });
        })
    };
    let filter_value = match *filter {
        Filter::All => "all",
        Filter::Used => "used",
        Filter::Unused => "unused",
        Filter::Issues => "issues",
    };

    html! {
        <div class="library">
            <div class="lib-toolbar">
                <input type="search" class="lib-search" value={(*query).clone()} oninput={on_query}
                    placeholder="Filter by ref, value, symbol, footprint, LCSC, MPN, badge..." />
                <select class="lib-filter" onchange={on_filter} value={filter_value}>
                    <option value="all" selected={filter_value == "all"}>{"All items"}</option>
                    <option value="used" selected={filter_value == "used"}>{"Used in design"}</option>
                    <option value="unused" selected={filter_value == "unused"}>{"Unused library items"}</option>
                    <option value="issues" selected={filter_value == "issues"}>{"Needs attention"}</option>
                </select>
                <span class="lib-stats">{stats}</span>
            </div>
            {for data.warnings.iter().map(|w| html! { <div class="lib-warn">{w}</div> })}
            if !data.libraries.is_empty() { <div class="lib-stats">{"Project libraries: "}{for libs}</div> }
            if !physical.is_empty() {
                <div class="lib-section">
                    <div class="lib-section-title">{"Used in design "}<span class="muted">{format!("{} parts", physical.len())}</span></div>
                    <div class="lib-grid">{physical.clone()}</div>
                </div>
            }
            if !power.is_empty() {
                <details class="lib-section lib-collapsible">
                    <summary class="lib-section-title">{"Power symbols "}<span class="muted">{format!("{} symbols", power.len())}</span></summary>
                    <div class="lib-grid">{power.clone()}</div>
                </details>
            }
            if !unused.is_empty() {
                <details class="lib-section lib-collapsible">
                    <summary class="lib-section-title">{"Unused library items "}<span class="muted">{format!("{} items", unused.len())}</span></summary>
                    <div class="lib-grid">{unused.clone()}</div>
                </details>
            }
            <details class="raw lib-legacy" ontoggle={on_links}>
                <summary>{"Reference link table"}</summary>
                <div class="lib-legacy-body">
                    {Html::from_html_unchecked(links.as_deref().unwrap_or("").to_string().into())}
                </div>
            </details>
            if let Some((part, kind)) = (*modal).clone() {
                <LibraryModal project={props.project.clone()} {part} {kind}
                    states={Rc::new((*states).clone())}
                    onclose={{ let modal = modal.clone(); Callback::from(move |_| modal.set(None)) }} />
            }
        </div>
    }
}

#[derive(Properties, PartialEq)]
struct ModalProps {
    project: AttrValue,
    part: Rc<LibraryPart>,
    kind: String,
    states: Rc<BTreeMap<String, ThumbStatus>>,
    onclose: Callback<()>,
}

#[function_component(LibraryModal)]
fn library_modal(props: &ModalProps) -> Html {
    let view = use_state(|| props.kind.clone());
    let scene = use_state(|| None::<Rc<vector_view::Scene>>);
    let selected = use_state(|| None::<vector_view::ItemId>);
    let failure = use_state(|| None::<String>);
    let part = props.part.clone();
    let states = props.states.clone();
    let current = (*view).clone();
    let t = (current != "render").then(|| effective(thumb(&part, &current), &states));

    {
        let scene = scene.clone();
        let selected = selected.clone();
        let failure = failure.clone();
        use_effect_with((current.clone(), t.clone()), move |(view, t)| {
            scene.set(None);
            selected.set(None);
            failure.set(None);
            if view != "model" {
                if let Some(url) = t.as_ref().and_then(|thumb| thumb.scene.clone()) {
                    spawn_local(async move {
                        match api::get_json::<vector_view::Scene>(&url, "").await {
                            Ok(value) => scene.set(Some(Rc::new(value))),
                            Err(err) => failure.set(Some(format!("Viewer failed: {err}"))),
                        }
                    });
                }
            }
        });
    }

    let tabs = KINDS.iter().chain(&[("render", "Large render")]).map(|(id, label)| {
        let enabled = if *id == "render" {
            KINDS.iter().any(|(k, _)| effective(thumb(&part, k), &states).state == "ready")
        } else {
            thumb(&part, id).key.is_some()
        };
        let onclick = {
            let view = view.clone();
            let id = id.to_string();
            Callback::from(move |_: MouseEvent| view.set(id.clone()))
        };
        html! { <button class={classes!((current == *id).then_some("active"))} disabled={!enabled} {onclick}>{*label}</button> }
    });

    let stage = if current == "render" {
        let imgs: Vec<String> = KINDS
            .iter()
            .map(|(k, _)| effective(thumb(&part, k), &states))
            .filter(|t| t.state == "ready")
            .filter_map(|t| t.url)
            .collect();
        let columns = format!(
            "position:absolute;inset:0;display:grid;grid-template-columns:repeat({},1fr)",
            imgs.len().max(1)
        );
        html! { <div style={columns}>{for imgs.into_iter().map(|src| html! { <div style="position:relative"><img {src} /></div> })}</div> }
    } else {
        match &t {
            Some(t) if t.key.is_none() => {
                html! { <div class="lib-stage-msg">{t.message.clone().unwrap_or_else(|| "Nothing to show".into())}</div> }
            }
            Some(t) if current == "model" => html! {
                <>
                    if let Some(msg) = &*failure { <div class="lib-stage-msg">{msg}</div> }
                    <ModelView project={props.project.clone()}
                        revision={AttrValue::from(t.key.clone().unwrap_or_default())}
                        url={t.viewer.clone().map(AttrValue::from)} active={true} />
                </>
            },
            Some(t) if t.scene.is_some() => html! {
                <>
                    if let Some(msg) = &*failure { <div class="lib-stage-msg">{msg}</div> }
                    if let Some(value) = &*scene {
                        <VectorSceneCanvas scene={value.clone()} selected={*selected}
                            on_select={{let selected=selected.clone();Callback::from(move |id|selected.set(id))}} />
                    } else if (*failure).is_none() {
                        <div class="lib-stage-msg">{"Loading vector geometry…"}</div>
                    }
                </>
            },
            Some(t) => match (&t.url, t.state.as_str()) {
                (Some(url), "ready") => html! { <img src={url.clone()} /> },
                _ => html! { <div class="lib-stage-msg">{placeholder(t)}</div> },
            },
            None => Html::default(),
        }
    };

    let mut footer = vec![compact_refs(&part.refs), part.values.join(", ")];
    footer.extend(part.symbol.clone());
    if let Some(source) = t.as_ref().and_then(|t| t.source.as_ref()) {
        footer.push(format!("rendered from {source} copy"));
    }
    let footer = footer
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");

    let onclose = props.onclose.clone();
    let backdrop = {
        let onclose = onclose.clone();
        Callback::from(move |e: MouseEvent| {
            if e.target() == e.current_target() {
                onclose.emit(());
            }
        })
    };
    html! {
        <div class="lib-modal" onclick={backdrop}>
            <div class="lib-dialog" role="dialog" aria-modal="true">
                <header>
                    <h3>{&part.title}</h3>
                    <div class="lib-tabs">{for tabs}</div>
                    <button class="lib-close" onclick={Callback::from(move |_| onclose.emit(()))}>{"Close"}</button>
                </header>
                <div class="lib-stage">{stage}</div>
                <footer>{footer}</footer>
            </div>
        </div>
    }
}
