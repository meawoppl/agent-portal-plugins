//! Debounced, project-scoped file watcher.
//!
//! Each changed path is mapped to the project(s) whose sources or artifacts it
//! affects, so an edit in one board never refreshes (or rebuilds) another,
//! including a root "." project that contains nested board roots.

use std::{collections::HashSet, path::PathBuf};

use anyhow::Result;
use kicad_pcb_shared::ServerEvent;
use notify::{Config as NotifyConfig, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::{
    sync::mpsc,
    time::{timeout, Duration, Instant},
};

use crate::{
    revision::{self, EventScope},
    AppState,
};

/// Quiet period before refreshing viewer state. Builds have their own,
/// longer debounce (`build.debounceMs`).
const QUIET_PERIOD: Duration = Duration::from_millis(100);
/// Upper bound on batching during a continuous stream of events.
const MAX_BATCH: Duration = Duration::from_secs(2);

pub(crate) fn spawn(state: AppState) {
    tokio::spawn(async move {
        if let Err(err) = run(state).await {
            tracing::warn!(error = %err, "KiCad PCB file watcher stopped");
        }
    });
}

fn relevant(event: &notify::Event) -> bool {
    !matches!(event.kind, EventKind::Access(_) | EventKind::Other)
}

async fn run(state: AppState) -> Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let cwd = state.cwd.clone();
    let mut watcher = RecommendedWatcher::new(
        move |result| {
            let _ = tx.send(result);
        },
        NotifyConfig::default(),
    )?;
    watcher.watch(&cwd, RecursiveMode::Recursive)?;

    while let Some(result) = rx.recv().await {
        let mut paths: HashSet<PathBuf> = HashSet::new();
        match result {
            Ok(event) if relevant(&event) => paths.extend(event.paths),
            Ok(_) => continue,
            Err(err) => {
                tracing::debug!(error = %err, "ignored file watch error");
                continue;
            }
        }
        let started = Instant::now();
        loop {
            let remaining = MAX_BATCH.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                break;
            }
            match timeout(QUIET_PERIOD.min(remaining), rx.recv()).await {
                Ok(Some(Ok(event))) if relevant(&event) => paths.extend(event.paths),
                Ok(Some(_)) => {}
                Ok(None) => return Ok(()),
                Err(_) => break,
            }
        }

        for project in state.projects.clone() {
            let mut source = false;
            let mut artifact = false;
            for path in &paths {
                match revision::event_scope(&project, path) {
                    EventScope::Source => source = true,
                    EventScope::Artifact => artifact = true,
                    EventScope::None => {}
                }
            }
            if !source && !artifact {
                continue;
            }
            let previous = {
                let warmed = state.warmed.read().await;
                warmed
                    .get(&project.id)
                    .map(|item| item.source_revision.clone())
            };
            let Some(previous) = previous else {
                // Not opened in the workbench: nothing to refresh or build.
                continue;
            };
            match crate::refresh_project_viewer_state(&state, &project).await {
                Ok(warmed) if warmed.source_revision != previous => {
                    let _ = state.events.send(ServerEvent::Revision {
                        project: Some(project.id.clone()),
                        revision: warmed.source_revision,
                        previous_revision: Some(previous),
                        warmed_at_ms: warmed.warmed_at_ms,
                        reason: "watch".to_string(),
                    });
                }
                Ok(warmed) if artifact => {
                    state.builds.refresh_status(&project).await;
                    let _ = state.events.send(ServerEvent::Revision {
                        project: Some(project.id.clone()),
                        revision: warmed.source_revision,
                        previous_revision: Some(previous),
                        warmed_at_ms: warmed.warmed_at_ms,
                        reason: "artifact-watch".to_string(),
                    });
                }
                Ok(_) => {}
                Err(err) => {
                    tracing::warn!(error = %err, project = %project.id, "failed to refresh KiCad PCB viewer state")
                }
            }
        }
    }
    Ok(())
}
