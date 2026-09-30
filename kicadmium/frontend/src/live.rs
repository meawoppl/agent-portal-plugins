use crate::api;
use futures::StreamExt;
use gloo_net::websocket::{futures::WebSocket, Message};
use shared::ServerEvent;
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;

#[derive(Properties, PartialEq)]
pub struct Props {
    pub project: AttrValue,
    pub on_revision: Callback<String>,
}

#[function_component(LiveStatus)]
pub fn live_status(props: &Props) -> Html {
    let label = use_state(|| "connecting".to_owned());
    let class = use_state(|| "busy".to_owned());
    {
        let label = label.clone();
        let class = class.clone();
        let project = props.project.to_string();
        let on_revision = props.on_revision.clone();
        use_effect_with(project.clone(), move |_| {
            let label = label.clone();
            let class = class.clone();
            spawn_local(async move {
                if let Ok(health) = api::health().await {
                    label.set(if health.kicad_cli {
                        "live".into()
                    } else {
                        "viewer only · kicad-cli missing".into()
                    });
                    class.set(if health.ok {
                        "live".into()
                    } else {
                        "warn".into()
                    });
                }
                if let Ok(mut socket) = WebSocket::open(&api::events_ws_url(&project)) {
                    while let Some(Ok(Message::Text(text))) = socket.next().await {
                        if let Ok(event) = serde_json::from_str::<ServerEvent>(&text) {
                            match event {
                                ServerEvent::Revision {
                                    reason, revision, ..
                                } => {
                                    label.set(format!("updated · {reason}"));
                                    class.set("live".into());
                                    on_revision.emit(revision);
                                }
                                ServerEvent::Build { status, .. } => {
                                    label.set(if status.busy {
                                        "building".into()
                                    } else {
                                        "live".into()
                                    });
                                    class.set(if status.busy {
                                        "busy".into()
                                    } else {
                                        "live".into()
                                    });
                                    if let Ok(detail) = serde_wasm_bindgen::to_value(&status) {
                                        let init = web_sys::CustomEventInit::new();
                                        init.set_detail(&detail);
                                        if let Ok(event) =
                                            web_sys::CustomEvent::new_with_event_init_dict(
                                                "kicadmium-build",
                                                &init,
                                            )
                                        {
                                            let _ =
                                                web_sys::window().unwrap().dispatch_event(&event);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                class.set("dead".into());
                label.set("disconnected".into());
            });
            || ()
        });
    }
    html! { <span class={classes!("live-pill",(*class).clone())}><span class="live-dot" />{(*label).clone()}</span> }
}
