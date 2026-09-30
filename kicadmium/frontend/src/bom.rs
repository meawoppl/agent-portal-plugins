//! BOM tab: the backend renders BOM/CPL CSV artifacts as tables.

use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;

use crate::api;

#[derive(Properties, PartialEq)]
pub struct BomTabProps {
    pub project: AttrValue,
    #[prop_or_default]
    pub revision: AttrValue,
}

#[function_component(BomTab)]
pub fn bom_tab(props: &BomTabProps) -> Html {
    let body = use_state(|| Err::<String, String>("Loading BOM/assembly artifacts...".into()));
    {
        let body = body.clone();
        use_effect_with(
            (props.project.clone(), props.revision.clone()),
            move |(project, _)| {
                let project = project.to_string();
                spawn_local(async move {
                    body.set(
                        api::bom_html(&project)
                            .await
                            .map_err(|err| format!("Unable to load BOM: {err}")),
                    );
                });
            },
        );
    }
    html! {
        <div class="card">
            <h2>{"BOM / Assembly CSV"}</h2>
            {match &*body {
                Ok(html) => Html::from_html_unchecked(html.clone().into()),
                Err(msg) => html! { <p class="muted">{msg}</p> },
            }}
        </div>
    }
}
