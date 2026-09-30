use gloo_net::http::Request;
use shared::HealthResponse;
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;

#[function_component(App)]
fn app() -> Html {
    let health = use_state(|| None::<HealthResponse>);
    {
        let health = health.clone();
        use_effect_with((), move |_| {
            spawn_local(async move {
                if let Ok(resp) = Request::get("/healthz").send().await {
                    if let Ok(data) = resp.json::<HealthResponse>().await {
                        health.set(Some(data));
                    }
                }
            });
        });
    }
    html! {
        <main>
            <h1>{ "kicadmium" }</h1>
            <p>{ match (*health).as_ref() {
                Some(h) => format!("backend ok={} kicad-cli={}", h.ok, h.kicad_cli),
                None => "checking backend...".to_string(),
            }}</p>
        </main>
    }
}

fn main() {
    yew::Renderer::<App>::new().render();
}
