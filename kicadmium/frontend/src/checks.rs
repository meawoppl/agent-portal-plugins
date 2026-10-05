use crate::api;
use serde_json::{json, Value};
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;

#[derive(Properties, PartialEq)]
pub struct Props {
    pub project: AttrValue,
}
fn issues(v: &Value) -> Vec<Value> {
    if let Some(r) = v
        .get("report")
        .and_then(Value::as_str)
        .and_then(|s| serde_json::from_str(s).ok())
    {
        return issues(&r);
    }
    [
        "violations",
        "unconnected_items",
        "schematic_parity",
        "findings",
    ]
    .iter()
    .flat_map(|k| {
        v.get(k)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    })
    .collect()
}

#[derive(Properties, PartialEq)]
struct CardProps {
    title: AttrValue,
    kind: AttrValue,
    project: AttrValue,
    #[prop_or(false)]
    lint: bool,
}

#[function_component(CheckCard)]
fn card(props: &CardProps) -> Html {
    let result = use_state(|| None::<Value>);
    {
        let result = result.clone();
        let kind = props.kind.to_string();
        let project = props.project.to_string();
        let lint = props.lint;
        use_effect_with((kind.clone(), project.clone()), move |_| {
            result.set(None);
            let token = crate::request::RequestToken::default();
            let task_token = token.clone();
            spawn_local(async move {
                let value = if lint {
                    api::lint(&project).await
                } else {
                    api::check(&kind, &project)
                        .await
                        .and_then(|c| serde_json::to_value(c).map_err(|e| e.to_string()))
                };
                if task_token.current() {
                    result.set(Some(value.unwrap_or_else(|e| json!({"message":e}))));
                }
            });
            move || token.cancel()
        });
    }
    let value = result.as_ref();
    let list = value.map(issues).unwrap_or_default();
    let errors = list
        .iter()
        .filter(|i| i.get("severity").and_then(Value::as_str) == Some("error"))
        .count();
    let stale = value
        .and_then(|v| v.get("review_audit"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter(|a| {
                    matches!(
                        a.get("status").and_then(Value::as_str),
                        Some("orphaned" | "resolved" | "expired")
                    )
                })
                .count()
        })
        .unwrap_or_default();
    let lint_links = if props.lint {
        html! {
            <p class="lint-artifacts">
                <a href={api::url("/api/kicad/lint/contact-sheet", &props.project)} target="_blank">{"Findings contact sheet"}</a>
                {" · "}
                <a href={api::url("/api/kicad/lint/exceptions", &props.project)} target="_blank">{"Exceptions contact sheet"}</a>
                {if stale > 0 { html!{<span class="pill error">{format!("{stale} stale exceptions")}</span>} } else {Html::default()}}
            </p>
        }
    } else {
        Html::default()
    };
    html! {<article class="card check-card"><h2>{&props.title}</h2>{if let Some(value)=value{html!{<><div class="check-summary"><span class={classes!("pill",if errors>0{"error"}else{"ok"})}>{if errors>0{"needs attention"}else{"reviewed"}}</span><span class="pill">{format!("{} findings",list.len())}</span><span class="pill error">{format!("{errors} errors")}</span></div>{lint_links}<div class="issue-list">{for list.into_iter().take(100).map(|item|{let severity=item.get("severity").and_then(Value::as_str).unwrap_or("warning").to_owned();let title=item.get("message").or_else(||item.get("description")).or_else(||item.get("rule")).and_then(Value::as_str).unwrap_or("Finding").to_owned();html!{<Issue lint={props.lint} project={props.project.clone()} {title} {severity} {item}/>}})}</div><details class="raw"><summary>{"Raw report"}</summary><pre>{serde_json::to_string_pretty(value).unwrap_or_default()}</pre></details></>}}else{html!{<p>{"Loading…"}</p>}}}</article>}
}

#[function_component(Checks)]
pub fn checks(props: &Props) -> Html {
    html! {<div class="grid"><CheckCard title="Layout quality" kind="quality" project={props.project.clone()}/><CheckCard title="DRC" kind="drc" project={props.project.clone()}/><CheckCard title="ERC" kind="erc" project={props.project.clone()}/><CheckCard title="Heuristic lint" kind="lint" project={props.project.clone()} lint=true/></div>}
}

#[derive(Properties, PartialEq)]
struct IssueProps {
    lint: bool,
    project: AttrValue,
    title: String,
    severity: String,
    item: Value,
}

/// One finding; lint findings can be queued for agent review.
#[function_component(Issue)]
fn issue(props: &IssueProps) -> Html {
    let state = use_state(|| None::<Result<String, String>>);
    let enqueue = props.lint.then(|| {
        let (state, project, item, title) = (
            state.clone(),
            props.project.to_string(),
            props.item.clone(),
            props.title.clone(),
        );
        Callback::from(move |_| {
            let (state, project, item, title) =
                (state.clone(), project.clone(), item.clone(), title.clone());
            let rule = item.get("rule").and_then(Value::as_str).unwrap_or("lint");
            let key = item.get("key").and_then(Value::as_str).unwrap_or("");
            let suggestion = item.get("suggestion").and_then(Value::as_str).unwrap_or("");
            let body = format!(
                "kct lint finding `{rule}` (key {key}): {title}\n\nSuggested action: {suggestion}\n\nReview it with `kicadmium kct -- lint run` and either fix it or record a reasoned waiver."
            );
            let payload = json!({
                "title": crate::annotate::title_from(&format!("{rule}: {title}"), "kct lint finding"),
                "body": body,
                "context": {"plugin": "kicadmium", "project": project, "tab": "checks",
                    "kind": "kct-lint-finding", "finding": item, "authorizedRepair": false},
            });
            spawn_local(async move {
                let source = json!({"plugin": "kicadmium", "project": project});
                state.set(Some(crate::annotate::enqueue(source, vec![payload]).await));
            });
        })
    });
    html! {<article class="issue"><div class="issue-head"><strong>{&props.title}</strong><span class={classes!("severity",props.severity.clone())}>{&props.severity}</span></div>
        {match (&*state, enqueue) {
            (Some(Ok(msg)), _) => html!{<span class="annotate-ok">{format!("✓ {msg}")}</span>},
            (Some(Err(msg)), Some(cb)) => html!{<><button class="enqueue" onclick={cb}>{"Retry"}</button><span class="annotate-err">{msg.clone()}</span></>},
            (None, Some(cb)) => html!{<button class="enqueue" onclick={cb}>{"Send to agent for review"}</button>},
            _ => Html::default(),
        }}
    </article>}
}
