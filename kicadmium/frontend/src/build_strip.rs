//! Build pipeline strip: one pill per stage with a details popover, the
//! publish staleness badge, and Rebuild/Publish actions.

use gloo_timers::callback::Interval;
use shared::{BuildStatus, StageStatus};
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;

use crate::api;

#[derive(Properties, PartialEq)]
pub struct BuildStripProps {
    pub project: AttrValue,
    /// Latest status pushed over `/ws/events` (`ServerEvent::Build`).
    #[prop_or_default]
    pub live: Option<BuildStatus>,
}

fn seconds(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms}ms")
    } else if ms < 10_000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        format!("{:.0}s", ms as f64 / 1000.0)
    }
}

fn timing(stage: &StageStatus) -> String {
    match stage.state.as_str() {
        "running" => stage
            .started_ms
            .map(|start| seconds((js_sys::Date::now() as u64).saturating_sub(start)))
            .unwrap_or_default(),
        "ok" | "failed" => {
            let elapsed = stage.elapsed_ms.map(seconds).unwrap_or_default();
            if stage.cached {
                format!("{elapsed} cached")
            } else {
                elapsed
            }
        }
        _ => String::new(),
    }
}

#[function_component(BuildStrip)]
pub fn build_strip(props: &BuildStripProps) -> Html {
    let status = use_state(|| None::<BuildStatus>);
    let message = use_state(String::new);
    let busy_action = use_state(|| false);
    let tick = use_state(|| 0u32);

    {
        let status = status.clone();
        let message = message.clone();
        use_effect_with(props.project.clone(), move |project| {
            let project = project.to_string();
            spawn_local(async move {
                match api::build_status(&project).await {
                    Ok(next) => status.set(Some(next)),
                    Err(err) => message.set(format!("status failed: {err}")),
                }
            });
        });
    }
    {
        let status = status.clone();
        use_effect_with(props.live.clone(), move |live| {
            if let Some(live) = live {
                status.set(Some(live.clone()));
            }
        });
    }
    // Re-render running timers once a second while the pipeline is busy.
    {
        let tick = tick.clone();
        let busy = status.as_ref().is_some_and(|s| s.busy);
        use_effect_with(busy, move |busy| {
            let interval = busy.then(|| Interval::new(1000, move || tick.set(*tick + 1)));
            move || drop(interval)
        });
    }

    let Some(current) = (*status).clone() else {
        return html! { <div class="build-strip"><span class="bs-msg">{"Build status loading..."}</span></div> };
    };
    let project = props.project.to_string();

    let action = |kind: &'static str| {
        let project = project.clone();
        let status = status.clone();
        let message = message.clone();
        let busy_action = busy_action.clone();
        Callback::from(move |_: MouseEvent| {
            let project = project.clone();
            let status = status.clone();
            let message = message.clone();
            let busy_action = busy_action.clone();
            busy_action.set(true);
            spawn_local(async move {
                match kind {
                    "publish" => match api::build_publish(&project).await {
                        Ok(report) => message.set(format!(
                            "published {} files",
                            report.get("files").cloned().unwrap_or_default()
                        )),
                        Err(err) => message.set(err),
                    },
                    _ => match api::build_run(&project, true).await {
                        Ok(next) => {
                            status.set(Some(next));
                            message.set(String::new());
                        }
                        Err(err) => message.set(err),
                    },
                }
                if let Ok(next) = api::build_status(&project).await {
                    status.set(Some(next));
                }
                busy_action.set(false);
            });
        })
    };

    let stages = current.stages.iter().map(|stage| {
        let outputs = stage.outputs.iter().map(|file| {
            html! { <li><a href={api::build_artifact_url(&project, &stage.stage, file)}>{file}</a></li> }
        });
        let input: String = stage.input_key.chars().take(12).collect();
        html! {
            <details class={classes!("bs-stage", stage.state.clone())}>
                <summary title={stage.message.clone().unwrap_or_else(|| stage.state.clone())}>
                    {&stage.label}{" "}<span>{&stage.state}</span>{" "}<span class="bs-time">{timing(stage)}</span>
                </summary>
                <div class="bs-pop">
                    <div><strong>{&stage.label}</strong>{format!(" · {}", stage.state)}</div>
                    if let Some(msg) = &stage.message { <div class="bs-msg">{msg}</div> }
                    <div class="bs-msg">{format!("input {input}")}</div>
                    if !stage.outputs.is_empty() { <ul>{for outputs}</ul> }
                </div>
            </details>
        }
    });

    let publish = &current.publish;
    let badge = if !publish.published {
        html! { <span class="bs-badge none" title="No published fab outputs">{"not published"}</span> }
    } else if publish.stale {
        let stages = if publish.stale_stages.is_empty() {
            String::new()
        } else {
            format!(": {}", publish.stale_stages.join(", "))
        };
        html! { <span class="bs-badge stale" title={publish.reasons.join("\n")}>{format!("fab outputs stale{stages}")}</span> }
    } else {
        html! { <span class="bs-badge fresh" title={publish.manifest.clone().unwrap_or_default()}>{"fab outputs current"}</span> }
    };
    let ready = !current.busy
        && !current.stages.is_empty()
        && current
            .stages
            .iter()
            .all(|stage| stage.state == "ok" || stage.state == "failed");
    let revision: String = current.hashes.revision.chars().take(8).collect();
    let publish_title = if current.auto_publish {
        "Auto-publish is on. Copy the current build into the configured fab dirs"
    } else {
        "Copy the current build into the configured fab dirs"
    };

    html! {
        <div class="build-strip">
            <span class="bs-msg">{format!("Build {revision}")}</span>
            {for stages}
            <span class="bs-spacer" />
            {badge}
            <button type="button" disabled={*busy_action} onclick={action("rebuild")}
                title="Discard cached outputs for this revision and rebuild">{"Rebuild"}</button>
            <button type="button" disabled={*busy_action || !ready} onclick={action("publish")} title={publish_title}>
                {if current.auto_publish { "Publish (auto)" } else { "Publish" }}
            </button>
            if !message.is_empty() { <span class="bs-msg">{(*message).clone()}</span> }
        </div>
    }
}
