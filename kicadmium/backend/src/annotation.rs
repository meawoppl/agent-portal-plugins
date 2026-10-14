//! Bounded, revision-bound geometry context for queued PCB annotations.
use crate::{jobs, AppError, AppState};
use anyhow::{anyhow, Result};
use axum::{extract::State, Json};
use serde::Deserialize;
use serde_json::{json, Value};
#[derive(Deserialize)]
pub(crate) struct Request {
    project: Option<String>,
    revision: String,
    region: kct::region::Bounds,
}
fn in_region(v: &Value, b: kct::region::Bounds) -> bool {
    let p = v
        .get("at")
        .or_else(|| v.get("location"))
        .cloned()
        .unwrap_or_else(|| v["items"][0]["pos"].clone());
    let x = p["x"].as_f64().or_else(|| p[0].as_f64());
    let y = p["y"].as_f64().or_else(|| p[1].as_f64());
    matches!((x,y),(Some(x),Some(y)) if b.contains(kct::lint::model::Point{x,y}))
}
fn compact(inventory: kct::region::Inventory, lint: &Value, quality: &Value) -> Value {
    let mut rows = Vec::new();
    for item in &inventory.items {
        if item["kind"] == "footprint" {
            rows.push(format!(
                "{} {} at={} rot={} side={}",
                item["reference"], item["value"], item["at"], item["rotation"], item["side"]
            ));
        }
    }
    let mut nets = std::collections::BTreeSet::new();
    for item in &inventory.items {
        if let Some(net) = item["net"].as_str().filter(|n| !n.is_empty()) {
            nets.insert(format!(
                "net {net}: {}",
                item.get("layers").unwrap_or(&item["layer"])
            ));
        }
    }
    rows.extend(nets);
    for (name, report, key) in [
        ("lint", lint, "findings"),
        ("quality", quality, "violations"),
    ] {
        if let Some(findings) = report[key].as_array() {
            for f in findings.iter().filter(|f| in_region(f, inventory.region)) {
                rows.push(format!(
                    "{name} {} {}: {}",
                    f.get("rule")
                        .or_else(|| f.get("type"))
                        .unwrap_or(&Value::Null),
                    f["severity"],
                    f.get("message")
                        .or_else(|| f.get("description"))
                        .unwrap_or(&Value::Null)
                ));
            }
        }
    }
    let omitted = rows.len().saturating_sub(36);
    rows.truncate(36);
    let mut text = format!("Board SHA256: {}\n", inventory.source_sha256);
    text += &rows.join("\n");
    if omitted > 0 {
        text += &format!("\n{omitted} more regional records omitted");
    }
    json!({"schema":"kicadmium.annotation-region/v1","board_sha256":inventory.source_sha256,"region":inventory.region,"summary":text,"omitted":omitted})
}
pub(crate) async fn endpoint(
    State(state): State<AppState>,
    Json(request): Json<Request>,
) -> Result<Json<Value>, AppError> {
    request.region.validate()?;
    let project = crate::selected_project(&state, request.project.as_deref())?;
    let path = crate::pick_project_file(&project, "kicad_pcb")?
        .ok_or_else(|| anyhow!("project has no PCB"))?;
    let region = request.region;
    // Optional enrichment only: never run lint or schedule a quality build here.
    let context = tokio::task::spawn_blocking(move || -> Result<Value> {
        let hashes = crate::revision::project_hashes(&project)?.hashes;
        if hashes.revision != request.revision {
            return Err(anyhow!("annotation revision is no longer current"));
        }
        let source = std::fs::read_to_string(&path)?;
        let inventory = kct::region::query(&source, region, &[], &[], 0)?;
        let quality = state.builds.cached_check(&project, &hashes, jobs::Stage::Quality)
            .and_then(|check| serde_json::from_str::<Value>(&check.report).ok());
        if crate::revision::project_hashes(&project)?.hashes.revision != hashes.revision {
            return Err(anyhow!("board changed while collecting annotation context"));
        }
        let mut context = compact(inventory, &Value::Null, quality.as_ref().unwrap_or(&Value::Null));
        context["revision"] = json!(hashes.revision);
        context["analysis"] = json!({"lint":"not_requested", "quality": if quality.is_some() {"cached"} else {"unavailable"}});
        Ok(context)
    }).await.map_err(|e| anyhow!(e))??;
    Ok(Json(context))
}
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn enrichment_without_cache_never_runs_checks_and_rejects_stale_revision() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        std::fs::write(
            root.join("board.kicad_pcb"),
            "(kicad_pcb (layers (0 \"F.Cu\" signal) (31 \"B.Cu\" signal)))",
        )
        .unwrap();
        // Any attempt to run lint would fail on this deliberately invalid policy.
        std::fs::write(root.join("board.lint.json"), "invalid policy").unwrap();
        let project = crate::revision::tests_support::project(&root, "board", ".");
        let revision = crate::revision::project_hashes(&project)
            .unwrap()
            .hashes
            .revision;
        let (events, _) = tokio::sync::broadcast::channel(16);
        let builds = jobs::BuildEngine::new(
            jobs::BuildSettings::from_config(None),
            vec![project.clone()],
            None,
            None,
            events.clone(),
        );
        let state = AppState {
            cwd: root,
            session: "test".into(),
            projects: vec![project],
            warmed: Default::default(),
            events,
            builds,
        };
        let request = |revision| Request {
            project: Some("board".into()),
            revision,
            region: kct::region::Bounds {
                x0: 0.,
                y0: 0.,
                x1: 10.,
                y1: 10.,
            },
        };
        let response = endpoint(State(state.clone()), Json(request(revision)))
            .await
            .unwrap()
            .0;
        assert_eq!(response["analysis"]["quality"], "unavailable");
        assert_eq!(response["analysis"]["lint"], "not_requested");
        assert!(endpoint(State(state), Json(request("old".into())))
            .await
            .is_err());
    }
    #[test]
    fn context_is_bounded_and_filters_findings() {
        let source = "(kicad_pcb (layers (0 \"F.Cu\" signal) (31 \"B.Cu\" signal)))";
        let inventory = kct::region::query(
            source,
            kct::region::Bounds {
                x0: 0.,
                y0: 0.,
                x1: 10.,
                y1: 10.,
            },
            &[],
            &[],
            0,
        )
        .unwrap();
        let findings:Vec<_>=(0..100).map(|i|json!({"rule":format!("rule{i}"),"severity":"warning","at":{"x":1,"y":1},"message":"nearby"})).collect();
        let r = compact(
            inventory,
            &json!({"findings":findings}),
            &json!({"violations":[{"type":"outside","items":[{"pos":{"x":99,"y":99}}]}]}),
        );
        assert!(r["summary"].as_str().unwrap().lines().count() < 40);
        assert_eq!(r["omitted"], 64);
        assert!(!r["summary"].as_str().unwrap().contains("outside"));
    }
}
