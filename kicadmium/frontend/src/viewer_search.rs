//! Shared component/net lookup and compact viewer chrome.
use std::collections::{HashMap, HashSet};
use vector_view::{GroupKind, ItemId, Scene};
use yew::prelude::*;

#[derive(Default, Clone, PartialEq)]
pub struct Matches {
    pub items: HashSet<ItemId>,
    pub references: HashSet<String>,
    pub labels: Vec<String>,
}

/// Prefer an exact reference/name; otherwise use a literal substring (never regex).
pub fn find(scene: &Scene, query: &str) -> Matches {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Matches::default();
    }
    let groups: Vec<_> = scene
        .groups
        .iter()
        .filter(|g| matches!(g.kind, GroupKind::Footprint | GroupKind::Symbol))
        .collect();
    let group_names = |g: &vector_view::Group| {
        std::iter::once(g.label.as_str())
            .chain(
                g.props
                    .iter()
                    .filter(|p| p.key == "value")
                    .map(|p| p.value.as_str()),
            )
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    let exact = groups.iter().any(|g| {
        group_names(g)
            .iter()
            .any(|name| name.to_lowercase() == query)
    }) || scene.nets.iter().any(|n| n.name.to_lowercase() == query);
    let matches = |s: &str| {
        let s = s.to_lowercase();
        if exact {
            s == query
        } else {
            s.contains(&query)
        }
    };
    let group_ids: HashSet<_> = groups
        .iter()
        .filter(|g| group_names(g).iter().any(|name| matches(name)))
        .map(|g| g.id)
        .collect();
    let net_ids: HashSet<_> = scene
        .nets
        .iter()
        .filter(|n| matches(&n.name))
        .map(|n| n.id)
        .collect();
    let mut out = Matches::default();
    for g in &groups {
        if group_ids.contains(&g.id) {
            out.labels.push(g.label.clone());
            out.references.insert(g.label.clone());
        }
    }
    for n in &scene.nets {
        if net_ids.contains(&n.id) {
            out.labels.push(n.name.clone());
        }
    }
    let by_id: HashMap<_, _> = groups.iter().map(|g| (g.id, *g)).collect();
    for item in &scene.items {
        if item.group.is_some_and(|g| group_ids.contains(&g))
            || item.net.is_some_and(|n| net_ids.contains(&n))
        {
            out.items.insert(item.id);
            if let Some(g) = item.group.and_then(|id| by_id.get(&id)) {
                out.references.insert(g.label.clone());
            }
        }
    }
    out.labels.sort();
    out.labels.dedup();
    out
}

#[derive(Properties, PartialEq)]
pub struct SearchProps {
    pub query: String,
    pub onchange: Callback<String>,
    pub summary: String,
}
#[function_component(ViewerSearch)]
pub fn viewer_search(props: &SearchProps) -> Html {
    let expanded = use_state(|| false);
    let input = use_node_ref();
    let oninput = {
        let change = props.onchange.clone();
        Callback::from(move |e: InputEvent| {
            change.emit(
                e.target_unchecked_into::<web_sys::HtmlInputElement>()
                    .value(),
            )
        })
    };
    let clear = {
        let change = props.onchange.clone();
        Callback::from(move |_| change.emit(String::new()))
    };
    let onkeydown = {
        let change = props.onchange.clone();
        Callback::from(move |e: KeyboardEvent| {
            if e.key() == "Escape" {
                change.emit(String::new());
                e.stop_propagation();
            }
        })
    };
    let toggle = {
        let expanded = expanded.clone();
        Callback::from(move |_| expanded.set(!*expanded))
    };
    html! {<div class={classes!("viewer-search", (*expanded || !props.query.is_empty()).then_some("expanded"))}>
        <button class="viewer-search-toggle" onclick={toggle} aria-label="Toggle component and net search" aria-expanded={(*expanded || !props.query.is_empty()).to_string()}>{"Search"}</button>
        <div class="viewer-search-fields"><input ref={input} type="search" aria-label="Find component or net" placeholder="Component or net…" value={props.query.clone()} {oninput} {onkeydown}/>
        if !props.query.is_empty() { <button onclick={clear} aria-label="Clear search">{"×"}</button><span role="status">{&props.summary}</span> }
        </div>
    </div>}
}

#[cfg(test)]
mod tests {
    use super::*;
    use vector_view::{Group, Item, Net, Prim, Prop, Role, SceneKind};
    #[test]
    fn net_matches_geometry_and_connected_components_only() {
        let mut s = Scene::new(SceneKind::Pcb, true);
        s.groups.push(Group {
            id: 1,
            kind: GroupKind::Footprint,
            label: "U1".into(),
            props: vec![Prop::new("value", "TPS62160")],
        });
        s.nets.push(Net {
            id: 7,
            name: "VIN".into(),
        });
        for (id, group, net) in [(1, Some(1), Some(7)), (2, None, Some(7)), (3, None, None)] {
            s.items.push(Item {
                id,
                layer: 0,
                role: Role::Pad,
                prim: Prim::Circle {
                    center: [0., 0.],
                    radius: 1.,
                    fill: true,
                    stroke: 0.,
                },
                net,
                group,
                props: vec![],
            });
        }
        assert_eq!(find(&s, "vin").items, HashSet::from([1, 2]));
        assert_eq!(find(&s, "vin").references, HashSet::from(["U1".into()]));
        assert_eq!(find(&s, "tps").items, HashSet::from([1]));
        assert_eq!(find(&s, "U1").items, HashSet::from([1]));
    }
    #[test]
    fn exact_names_win_and_substrings_are_literal_case_insensitive() {
        let mut s = Scene::new(SceneKind::Pcb, true);
        for (id, label) in [(1, "U1"), (2, "U10"), (3, "J[1]")] {
            s.groups.push(Group {
                id,
                kind: GroupKind::Footprint,
                label: label.into(),
                props: vec![],
            });
        }
        s.nets.push(Net {
            id: 1,
            name: "USB_D+".into(),
        });
        assert_eq!(find(&s, " u1 ").labels, vec!["U1"]);
        assert_eq!(find(&s, "u").labels, vec!["U1", "U10", "USB_D+"]);
        assert_eq!(find(&s, "[1]").labels, vec!["J[1]"]);
        assert_eq!(find(&s, "d+").labels, vec!["USB_D+"]);
        assert!(find(&s, " ").labels.is_empty());
        assert!(find(&s, "missing").labels.is_empty());
    }
}
