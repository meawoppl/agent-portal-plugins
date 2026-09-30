use futures::StreamExt;
use gloo_net::{http::Request, websocket::{futures::WebSocket, Message}};
use shared::{HealthResponse, ServerEvent};
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;

#[derive(Properties, PartialEq)]
pub struct Props { pub project: AttrValue }

#[function_component(LiveStatus)]
pub fn live_status(props:&Props)->Html{
    let label=use_state(||"connecting".to_owned()); let class=use_state(||"busy".to_owned());
    {let label=label.clone();let class=class.clone();let project=props.project.to_string();use_effect_with(project,move |_|{
        let label=label.clone();let class=class.clone();spawn_local(async move{
            if let Ok(r)=Request::get("/healthz").send().await{if let Ok(h)=r.json::<HealthResponse>().await{label.set(if h.kicad_cli{"live".into()}else{"viewer only · kicad-cli missing".into()});class.set(if h.ok{"live".into()}else{"warn".into()});}}
            let scheme=if web_sys::window().unwrap().location().protocol().ok().as_deref()==Some("https:"){"wss"}else{"ws"};
            let host=web_sys::window().unwrap().location().host().unwrap_or_default();
            if let Ok(mut ws)=WebSocket::open(&format!("{scheme}://{host}/ws/events")){while let Some(Ok(Message::Text(text)))=ws.next().await{if let Ok(event)=serde_json::from_str::<ServerEvent>(&text){match event{ServerEvent::Revision{reason,..}=>{label.set(format!("updated · {reason}"));class.set("live".into())},ServerEvent::Build{status,..}=>{label.set(if status.busy{"building".into()}else{"live".into()});class.set(if status.busy{"busy".into()}else{"live".into())}}}}}}
            class.set("dead".into());label.set("disconnected".into());
        });||()});}
    html!{<span class={classes!("live-pill",(*class).clone())}><span class="live-dot"/>{(*label).clone()}</span>}
}
