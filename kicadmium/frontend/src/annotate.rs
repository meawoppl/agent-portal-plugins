use gloo_net::http::Request;
use serde_json::json;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;
use web_sys::HtmlInputElement;
use yew::prelude::*;

#[derive(Properties,PartialEq)] pub struct Props{pub project:AttrValue,pub tab:AttrValue,pub revision:AttrValue}

#[function_component(Annotator)] pub fn annotator(props:&Props)->Html{
    let open=use_state(||false);let note=use_state(String::new);let status=use_state(String::new);
    let toggle={let open=open.clone();Callback::from(move |_|open.set(!*open))};
    let input={let note=note.clone();Callback::from(move|e:InputEvent|note.set(e.target_unchecked_into::<HtmlInputElement>().value()))};
    let send={let note=note.clone();let status=status.clone();let project=props.project.to_string();let tab=props.tab.to_string();let revision=props.revision.to_string();Callback::from(move |_|{if note.trim().is_empty(){return}let payload=json!({"kind":"kicadmium-annotation","project":project,"tab":tab,"revision":revision,"selection":{"x":0,"y":0,"width":1,"height":1},"note":*note,"authorizedRepair":false});let status=status.clone();spawn_local(async move{match Request::post("/__portal/edit-stack").header("content-type","application/json").body(payload.to_string()).unwrap().send().await{Ok(r)if r.ok()=>status.set("Queued for agent review".into()),Ok(r)=>status.set(format!("Queue unavailable (HTTP {})",r.status())),Err(_)=>status.set("Queue bridge is available only inside Agent Portal".into())}});})};
    html!{<div class="annotator"><button onclick={toggle}> {if *open{"Close annotation"}else{"Annotate"}} </button>{if *open{html!{<div class="annotation-composer"><label>{format!("{} · {}",props.tab,props.revision)}</label><input placeholder="Describe the selected region or requested change" value={(*note).clone()} oninput={input}/><button onclick={send}>{"Send to queue"}</button><span class="muted">{(*status).clone()}</span></div>}}else{Html::default()}}</div>}
}
