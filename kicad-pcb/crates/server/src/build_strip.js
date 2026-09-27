// KiCad PCB build status strip. Rendered into #buildStrip; updated from
// `kicad-pcb-build` window events (dispatched by the workbench WebSocket
// handler) and /api/build/status.
(() => {
  const root = document.getElementById("buildStrip");
  if (!root) return;
  const projectId = root.dataset.project || "";
  const q = `project=${encodeURIComponent(projectId)}`;
  const esc = value => String(value ?? "").replace(/[&<>"']/g, c => ({"&":"&amp;","<":"&lt;",">":"&gt;","\"":"&quot;","'":"&#39;"}[c]));
  const style = document.createElement("style");
  style.textContent = `
#buildStrip{display:flex;flex-wrap:wrap;align-items:center;gap:6px;padding:6px 8px;background:#1a1b26;border-bottom:1px solid #3b4261;font-size:12px}
#buildStrip .bs-stage{position:relative}
#buildStrip .bs-stage summary{list-style:none;cursor:pointer;border:1px solid #3b4261;border-radius:999px;padding:3px 8px;background:#24283b;white-space:nowrap}
#buildStrip .bs-stage summary::-webkit-details-marker{display:none}
#buildStrip .bs-stage[open] summary{outline:1px solid #7aa2f7}
#buildStrip .bs-pop{position:absolute;z-index:5;top:26px;left:0;min-width:240px;max-width:420px;background:#1f2335;border:1px solid #3b4261;border-radius:6px;padding:8px;box-shadow:0 4px 16px #0008}
#buildStrip .bs-pop ul{margin:4px 0 0;padding-left:16px}
#buildStrip .ok summary{border-color:#9ece6a;color:#9ece6a}
#buildStrip .failed summary{border-color:#f7768e;color:#f7768e}
#buildStrip .running summary{border-color:#7aa2f7;color:#7aa2f7}
#buildStrip .queued summary,#buildStrip .stale summary,#buildStrip .cancelled summary{border-color:#565f89;color:#9aa5ce}
#buildStrip .stale summary{border-style:dashed}
#buildStrip .bs-badge{border-radius:4px;padding:3px 7px;font-weight:600}
#buildStrip .bs-badge.stale{background:#e0af6822;color:#e0af68;border:1px solid #e0af68}
#buildStrip .bs-badge.fresh{background:#9ece6a18;color:#9ece6a;border:1px solid #9ece6a55}
#buildStrip .bs-badge.none{color:#9aa5ce;border:1px solid #3b4261}
#buildStrip button{border:1px solid #3b4261;color:#c0caf5;background:#24283b;padding:3px 9px;border-radius:6px;cursor:pointer;font-size:12px}
#buildStrip button:disabled{opacity:.5;cursor:default}
#buildStrip .bs-spacer{flex:1}
#buildStrip .bs-msg{color:#9aa5ce}`;
  document.head.append(style);

  let status;
  let message = "";
  const glyph = {ok:"ok", failed:"failed", running:"running", queued:"queued", stale:"stale", cancelled:"cancelled"};
  const seconds = ms => ms == null ? "" : ms < 1000 ? `${ms}ms` : `${(ms / 1000).toFixed(ms < 10000 ? 1 : 0)}s`;
  const timing = stage => {
    if (stage.state === "running" && stage.started_ms) return seconds(Date.now() - stage.started_ms);
    if (stage.state === "ok" || stage.state === "failed") return stage.cached ? `${seconds(stage.elapsed_ms)} cached` : seconds(stage.elapsed_ms);
    return "";
  };
  const artifactUrl = (stage, file) => `/api/build/artifact?${q}&stage=${encodeURIComponent(stage)}&file=${encodeURIComponent(file)}&download=1`;
  const render = () => {
    if (!status) { root.innerHTML = `<span class="bs-msg">Build status loading...</span>`; return; }
    const openStage = root.querySelector("details[open]")?.dataset.stage;
    const stages = (status.stages || []).map(stage => {
      const outputs = (stage.outputs || []).map(file => `<li><a href="${artifactUrl(stage.stage, file)}">${esc(file)}</a></li>`).join("");
      const detail = `<div><strong>${esc(stage.label)}</strong> · ${esc(stage.state)}</div>${stage.message ? `<div class="bs-msg">${esc(stage.message)}</div>` : ""}<div class="bs-msg">input ${esc((stage.input_key || "").slice(0, 12))}</div>${outputs ? `<ul>${outputs}</ul>` : ""}`;
      return `<details class="bs-stage ${esc(stage.state)}" data-stage="${esc(stage.stage)}"${openStage === stage.stage ? " open" : ""}><summary title="${esc(stage.message || stage.state)}">${esc(stage.label)} <span>${esc(glyph[stage.state] || stage.state)}</span> <span class="bs-time">${esc(timing(stage))}</span></summary><div class="bs-pop">${detail}</div></details>`;
    }).join("");
    const publish = status.publish || {};
    let badge;
    if (!publish.published) badge = `<span class="bs-badge none" title="No published fab outputs">not published</span>`;
    else if (publish.stale) badge = `<span class="bs-badge stale" title="${esc((publish.reasons || []).join("\n"))}">fab outputs stale${publish.stale_stages?.length ? `: ${esc(publish.stale_stages.join(", "))}` : ""}</span>`;
    else badge = `<span class="bs-badge fresh" title="${esc(publish.manifest || "")}">fab outputs current</span>`;
    const ready = !status.busy && (status.stages || []).length && (status.stages || []).every(stage => stage.state === "ok" || stage.state === "failed");
    root.innerHTML = `<span class="bs-msg">Build ${esc((status.hashes?.revision || "").slice(0, 8))}</span>${stages}<span class="bs-spacer"></span>${badge}<button type="button" data-action="rebuild" title="Discard cached outputs for this revision and rebuild">Rebuild</button><button type="button" data-action="publish" ${ready ? "" : "disabled"} title="${status.auto_publish ? "Auto-publish is on. " : ""}Copy the current build into the configured fab dirs">Publish${status.auto_publish ? " (auto)" : ""}</button>${message ? `<span class="bs-msg">${esc(message)}</span>` : ""}`;
  };
  const load = async () => {
    try { status = await fetch(`/api/build/status?${q}`).then(r => r.json()); } catch (err) { message = `status failed: ${err.message || err}`; }
    render();
  };
  root.addEventListener("click", async event => {
    const action = event.target?.dataset?.action;
    if (!action) return;
    event.target.disabled = true;
    try {
      if (action === "publish") {
        const response = await fetch(`/api/build/publish?${q}`, {method:"POST"});
        const payload = await response.json();
        message = response.ok ? `published ${payload.files} files` : (payload.message || `HTTP ${response.status}`);
      } else if (action === "rebuild") {
        status = await fetch(`/api/build/run?${q}&force=1`, {method:"POST"}).then(r => r.json());
        message = "";
      }
    } catch (err) { message = String(err.message || err); }
    await load();
  });
  window.addEventListener("kicad-pcb-build", event => {
    if (event.detail?.project !== projectId) return;
    status = event.detail.status;
    render();
  });
  setInterval(() => { if (status?.busy) render(); }, 1000);
  window.kicadPcbBuildStatus = () => status;
  render();
  void load();
})();
