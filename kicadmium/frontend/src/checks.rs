use crate::api;
use gloo_net::http::Request;
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
            spawn_local(async move {
                let value = if lint {
                    api::lint(&project).await
                } else {
                    api::check(&kind, &project)
                        .await
                        .and_then(|c| serde_json::to_value(c).map_err(|e| e.to_string()))
                };
                result.set(Some(value.unwrap_or_else(|e| json!({"message":e}))));
            });
            || ()
        });
    }
    let value = result.as_ref();
    let list = value.map(issues).unwrap_or_default();
    let errors = list
        .iter()
        .filter(|i| i.get("severity").and_then(Value::as_str) == Some("error"))
        .count();
    html! {<article class="card check-card"><h2>{&props.title}</h2>{if let Some(value)=value{html!{<><div class="check-summary"><span class={classes!("pill",if errors>0{"error"}else{"ok"})}>{if errors>0{"needs attention"}else{"reviewed"}}</span><span class="pill">{format!("{} findings",list.len())}</span><span class="pill error">{format!("{errors} errors")}</span></div><div class="issue-list">{for list.into_iter().take(100).map(|item|{let severity=item.get("severity").and_then(Value::as_str).unwrap_or("warning").to_owned();let title=item.get("message").or_else(||item.get("description")).or_else(||item.get("rule")).and_then(Value::as_str).unwrap_or("Finding").to_owned();let enqueue=if props.lint{let project=props.project.to_string();Some(Callback::from(move |_|{let body=json!({"kind":"pcb-lint-finding","project":project,"finding":item,"authorizedRepair":false});spawn_local(async move{let _=Request::post("/__portal/edit-stack").header("content-type","application/json").body(body.to_string()).unwrap().send().await;});}))}else{None};html!{<article class="issue"><div class="issue-head"><strong>{title}</strong><span class={classes!("severity",severity.clone())}>{severity}</span></div>{enqueue.map(|cb|html!{<button class="enqueue" onclick={cb}>{"Enqueue for review"}</button>}).unwrap_or_default()}</article>}})}</div><details class="raw"><summary>{"Raw report"}</summary><pre>{serde_json::to_string_pretty(value).unwrap_or_default()}</pre></details></>}}else{html!{<p>{"Loading…"}</p>}}}</article>}
}

#[function_component(Checks)]
pub fn checks(props: &Props) -> Html {
    html! {<div class="grid"><CheckCard title="Layout quality" kind="quality" project={props.project.clone()}/><CheckCard title="DRC" kind="drc" project={props.project.clone()}/><CheckCard title="ERC" kind="erc" project={props.project.clone()}/><CheckCard title="Heuristic lint" kind="lint" project={props.project.clone()} lint=true/></div>}
}
