// KiCad PCB smart library view. Served from /api/kicad/library/ui.js and
// mounted by the workbench "Libraries" tab via KicadLibrary.load(el, api).
(() => {
  "use strict";
  const POLL_MS = 900;
  const state = {
    el: null,
    api: path => path,
    data: null,
    signature: "",
    query: "",
    filter: "all",
    visible: new Set(),
    observer: null,
    timer: null,
    polling: false,
  };

  const esc = value =>
    String(value ?? "").replace(/[&<>"']/g, c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
  const KINDS = [
    ["symbol", "Symbol"],
    ["footprint", "Footprint"],
    ["model", "3D"],
  ];

  const css = `
.lib-toolbar{display:flex;flex-wrap:wrap;gap:8px;align-items:center;margin:0 0 12px}
.lib-toolbar input{flex:1 1 260px;min-width:180px;border:1px solid #3b4261;border-radius:6px;background:#1a1b26;color:#c0caf5;padding:8px 10px}
.lib-toolbar select{padding:7px 10px}
.lib-stats{color:#9aa5ce;font-size:12px}
.lib-section-title{margin:18px 0 8px;font-size:14px;color:#e6e9f5;display:flex;gap:8px;align-items:baseline}
.lib-section-title .muted{font-weight:normal;font-size:12px}
.lib-grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(360px,1fr));gap:12px}
.lib-card{border:1px solid #3b4261;border-radius:8px;background:#1a1b26;overflow:hidden;display:flex;flex-direction:column}
.lib-card.hidden{display:none}
.lib-trip{display:grid;grid-template-columns:repeat(3,1fr);gap:1px;background:#292e42;border-bottom:1px solid #3b4261}
.lib-cell{position:relative;aspect-ratio:4/3;background:#1a1b26;border:0;padding:6px;margin:0;cursor:pointer;display:flex;align-items:center;justify-content:center;color:#565f89;font:inherit;font-size:11px}
.lib-cell:hover{background:#1f2335}
.lib-cell:disabled{cursor:default}
.lib-cell img{max-width:100%;max-height:100%;width:100%;height:100%;object-fit:contain;display:block}
.lib-cell .ph{padding:4px;text-align:center;line-height:1.3}
.lib-cell[data-state=queued] .ph,.lib-cell[data-state=rendering] .ph{animation:libpulse 1.4s ease-in-out infinite}
.lib-cell[data-state=failed] .ph{color:#f7768e}
.lib-cell .lab{position:absolute;left:6px;bottom:4px;font-size:10px;color:#565f89;letter-spacing:.04em;text-transform:uppercase}
@keyframes libpulse{50%{opacity:.35}}
.lib-body{padding:10px 12px;display:grid;gap:4px;font-size:12px}
.lib-title{font-size:13px;color:#e6e9f5;font-weight:600;overflow-wrap:anywhere}
.lib-sub{color:#7dcfff;font-family:ui-monospace,monospace;overflow-wrap:anywhere}
.lib-meta{color:#9aa5ce;overflow-wrap:anywhere}
.lib-meta strong{color:#c0caf5;font-weight:600}
.lib-desc{display:-webkit-box;-webkit-line-clamp:2;-webkit-box-orient:vertical;overflow:hidden}
.lib-links{display:flex;flex-wrap:wrap;gap:8px}
.lib-badges{display:flex;flex-wrap:wrap;gap:4px;margin-top:2px}
.lib-badge{border:1px solid #3b4261;border-radius:999px;padding:1px 7px;font-size:11px;background:#24283b}
.lib-badge.error{border-color:#f7768e;color:#f7768e}
.lib-badge.warning{border-color:#e0af68;color:#e0af68}
.lib-badge.info{border-color:#565f89;color:#9aa5ce}
.lib-badge.kind-jlc-correction.info{border-color:#7aa2f7;color:#7aa2f7}
.lib-warn{color:#e0af68;margin:4px 0}
.lib-modal{position:fixed;inset:0;background:#0b0c12d9;z-index:50;display:flex;align-items:center;justify-content:center;padding:24px}
.lib-dialog{width:min(1100px,100%);height:min(820px,100%);background:#1a1b26;border:1px solid #3b4261;border-radius:10px;display:flex;flex-direction:column;overflow:hidden}
.lib-dialog header{display:flex;gap:10px;align-items:center;justify-content:space-between;padding:10px 12px;border-bottom:1px solid #3b4261;background:#1f2335}
.lib-dialog header h3{margin:0;font-size:14px;color:#e6e9f5;overflow-wrap:anywhere}
.lib-dialog .lib-tabs{display:flex;gap:4px}
.lib-dialog .lib-tabs button,.lib-dialog .lib-close{border:1px solid #3b4261;color:#c0caf5;background:#24283b;padding:5px 10px;border-radius:6px;cursor:pointer}
.lib-dialog .lib-tabs button.active{background:#7aa2f7;color:#10131d;border-color:#7aa2f7}
.lib-dialog .lib-tabs button:disabled{opacity:.4;cursor:default}
.lib-stage{flex:1;position:relative;background:#11131d}
.lib-stage iframe{position:absolute;inset:0;width:100%;height:100%;border:0;border-radius:0}
.lib-stage img{position:absolute;inset:0;width:100%;height:100%;object-fit:contain;padding:24px;box-sizing:border-box}
.lib-stage .lib-stage-msg{position:absolute;inset:0;display:flex;align-items:center;justify-content:center;color:#9aa5ce}
.lib-dialog footer{padding:6px 12px;border-top:1px solid #3b4261;color:#9aa5ce;font-size:12px;display:flex;gap:12px;flex-wrap:wrap}
`;

  const ensureStyle = () => {
    if (document.getElementById("kicad-library-style")) return;
    const style = document.createElement("style");
    style.id = "kicad-library-style";
    style.textContent = css;
    document.head.append(style);
  };

  const compactRefs = refs => {
    if (!refs?.length) return "";
    const shown = refs.slice(0, 10).join(", ");
    return refs.length > 10 ? `${shown} +${refs.length - 10}` : shown;
  };

  const lcscLink = code => {
    if (!code) return "";
    const clean = String(code).trim();
    const href = /^C\d+$/i.test(clean)
      ? `https://www.lcsc.com/product-detail/${encodeURIComponent(clean.toUpperCase())}.html`
      : `https://www.lcsc.com/search?q=${encodeURIComponent(clean)}`;
    return `<a href="${esc(href)}" target="_blank" rel="noopener">LCSC ${esc(clean)}</a>`;
  };

  const placeholderText = thumb => {
    switch (thumb.state) {
      case "queued":
        return "queued";
      case "rendering":
        return "rendering...";
      case "failed":
        return "render failed";
      case "idle":
        return "waiting";
      default:
        return thumb.message || "n/a";
    }
  };

  const cellHtml = (part, kind, label) => {
    const thumb = part.thumbs?.[kind] || { state: "none" };
    const inner =
      thumb.state === "ready" && thumb.url
        ? `<img loading="lazy" alt="${esc(label)}" src="${esc(thumb.url)}">`
        : `<span class="ph">${esc(placeholderText(thumb))}</span>`;
    const title = thumb.message ? ` title="${esc(thumb.message)}"` : "";
    const disabled = thumb.key ? "" : " disabled";
    return `<button class="lib-cell" data-kind="${kind}" data-key="${esc(thumb.key || "")}" data-state="${esc(thumb.state)}"${title}${disabled}>${inner}<span class="lab">${esc(label)}</span></button>`;
  };

  const searchText = part =>
    [part.title, part.symbol, part.footprint, ...(part.refs || []), ...(part.values || []), part.lcsc, part.mpn, part.manufacturer, part.description, ...(part.badges || []).map(b => b.label)]
      .filter(Boolean)
      .join(" ")
      .toLowerCase();

  const hasIssue = part => (part.badges || []).some(b => b.level === "error" || b.level === "warning");

  const cardHtml = (part, section) => {
    const badges = (part.badges || [])
      .map(b => `<span class="lib-badge ${esc(b.level)} kind-${esc(b.kind)}"${b.detail ? ` title="${esc(b.detail)}"` : ""}>${esc(b.label)}</span>`)
      .join("");
    const sub = part.footprint && part.symbol ? part.symbol : "";
    const meta = [];
    if (part.refs?.length) meta.push(`<strong>${esc(compactRefs(part.refs))}</strong>`);
    if (part.values?.length) meta.push(esc(part.values.join(", ")));
    const links = [];
    if (part.lcsc) links.push(lcscLink(part.lcsc));
    if (part.mpn) links.push(`<span>${esc([part.manufacturer, part.mpn].filter(Boolean).join(" "))}</span>`);
    if (part.datasheet && /^https?:/i.test(part.datasheet)) links.push(`<a href="${esc(part.datasheet)}" target="_blank" rel="noopener">datasheet</a>`);
    return `<article class="lib-card" data-id="${esc(part.id)}" data-section="${section}" data-issue="${hasIssue(part) ? 1 : 0}" data-search="${esc(searchText(part))}">
<div class="lib-trip">${KINDS.map(([kind, label]) => cellHtml(part, kind, label)).join("")}</div>
<div class="lib-body"><div class="lib-title">${esc(part.title)}</div>${sub ? `<div class="lib-sub">${esc(sub)}</div>` : ""}${meta.length ? `<div class="lib-meta">${meta.join(" · ")}</div>` : ""}${part.description ? `<div class="lib-meta lib-desc" title="${esc(part.description)}">${esc(part.description)}</div>` : ""}${links.length ? `<div class="lib-links">${links.join("")}</div>` : ""}${badges ? `<div class="lib-badges">${badges}</div>` : ""}</div></article>`;
  };

  const allParts = () => [...(state.data?.parts || []), ...(state.data?.unused || [])];
  const partById = id => allParts().find(part => part.id === id);

  const statsText = () => {
    const s = state.data?.stats || {};
    const pending = document.querySelectorAll(".lib-cell[data-state=queued], .lib-cell[data-state=rendering]").length;
    const ready = document.querySelectorAll(".lib-cell[data-state=ready]").length;
    return `${s.parts ?? 0} parts · ${s.unused ?? 0} unused library items · ${ready}/${s.thumbs ?? 0} renders ready${pending ? ` · ${pending} rendering` : ""} · KiCad ${esc(state.data?.kicad_version || "?")}`;
  };

  const applyFilter = () => {
    const query = state.query.trim().toLowerCase();
    const terms = query.split(/\s+/).filter(Boolean);
    for (const card of state.el.querySelectorAll(".lib-card")) {
      const text = card.dataset.search || "";
      const matchesQuery = terms.every(term => text.includes(term));
      const section = card.dataset.section;
      const matchesFilter =
        state.filter === "all" ||
        (state.filter === "used" && section === "used") ||
        (state.filter === "unused" && section === "unused") ||
        (state.filter === "issues" && card.dataset.issue === "1");
      card.classList.toggle("hidden", !(matchesQuery && matchesFilter));
    }
    for (const section of state.el.querySelectorAll(".lib-section")) {
      const shown = section.querySelectorAll(".lib-card:not(.hidden)").length;
      section.style.display = shown ? "" : "none";
      const count = section.querySelector(".lib-count");
      if (count) count.textContent = `${shown}`;
    }
  };

  const render = () => {
    ensureStyle();
    const data = state.data;
    const warnings = (data.warnings || []).map(w => `<div class="lib-warn">${esc(w)}</div>`).join("");
    const libs = (data.libraries || []).map(lib => `<code>${esc(lib.nickname)}</code> ${esc(lib.kind)} (${lib.items})`).join(" · ");
    state.el.innerHTML = `<div class="lib-toolbar"><input type="search" class="lib-search" placeholder="Filter by ref, value, symbol, footprint, LCSC, MPN, badge..." value="${esc(state.query)}">
<select class="lib-filter"><option value="all">All items</option><option value="used">Used in design</option><option value="unused">Unused library items</option><option value="issues">Needs attention</option></select>
<span class="lib-stats"></span></div>${warnings}${libs ? `<div class="lib-stats">Project libraries: ${libs}</div>` : ""}
<div class="lib-section" data-section="used"><div class="lib-section-title">Used in design <span class="muted"><span class="lib-count"></span> parts</span></div><div class="lib-grid">${(data.parts || []).map(p => cardHtml(p, "used")).join("")}</div></div>
<div class="lib-section" data-section="unused"><div class="lib-section-title">Unused library items <span class="muted"><span class="lib-count"></span> items</span></div><div class="lib-grid">${(data.unused || []).map(p => cardHtml(p, "unused")).join("")}</div></div>
<details class="raw lib-legacy"><summary>Reference link table</summary><div class="lib-legacy-body muted">Loading...</div></details>`;
    const search = state.el.querySelector(".lib-search");
    search.addEventListener("input", () => {
      state.query = search.value;
      applyFilter();
    });
    const filter = state.el.querySelector(".lib-filter");
    filter.value = state.filter;
    filter.addEventListener("change", () => {
      state.filter = filter.value;
      applyFilter();
    });
    state.el.querySelector(".lib-legacy").addEventListener("toggle", async event => {
      const body = event.target.querySelector(".lib-legacy-body");
      if (!event.target.open || body.dataset.loaded) return;
      body.dataset.loaded = "1";
      try {
        body.innerHTML = await fetch(state.api("/api/kicad/libraries")).then(r => r.text());
        body.classList.remove("muted");
      } catch (err) {
        body.textContent = `Unable to load: ${err.message}`;
      }
    });
    state.el.querySelectorAll(".lib-cell").forEach(cell =>
      cell.addEventListener("click", () => {
        const card = cell.closest(".lib-card");
        const part = partById(card.dataset.id);
        if (part) openModal(part, cell.dataset.kind);
      }),
    );
    observe();
    applyFilter();
    updateStats();
  };

  const updateStats = () => {
    const el = state.el?.querySelector(".lib-stats");
    if (el) el.innerHTML = statsText();
  };

  const observe = () => {
    state.observer?.disconnect();
    state.visible.clear();
    state.observer = new IntersectionObserver(
      entries => {
        for (const entry of entries) {
          if (entry.isIntersecting) state.visible.add(entry.target);
          else state.visible.delete(entry.target);
        }
        schedulePoll(50);
      },
      { rootMargin: "200px" },
    );
    state.el.querySelectorAll(".lib-card").forEach(card => state.observer.observe(card));
  };

  const pendingCells = () => [...state.el.querySelectorAll(".lib-cell[data-state=queued], .lib-cell[data-state=rendering]")];

  const setCell = (cell, status) => {
    const key = cell.dataset.key;
    const part = partById(cell.closest(".lib-card")?.dataset.id);
    const thumb = part?.thumbs?.[cell.dataset.kind];
    if (thumb && thumb.key === key) {
      thumb.state = status.state;
      thumb.message = status.message || thumb.message;
      if (status.url) thumb.url = status.url;
    }
    if (cell.dataset.state === status.state) return;
    cell.dataset.state = status.state;
    if (status.state === "ready" && status.url) {
      const img = new Image();
      img.alt = cell.dataset.kind;
      img.onload = () => cell.replaceChildren(img, cell.querySelector(".lab") || "");
      img.src = status.url;
    } else {
      const ph = cell.querySelector(".ph");
      if (ph) ph.textContent = placeholderText(status);
      if (status.message) cell.title = status.message;
    }
  };

  const schedulePoll = delay => {
    if (state.timer) return;
    state.timer = setTimeout(() => {
      state.timer = null;
      void poll();
    }, delay);
  };

  const poll = async () => {
    if (state.polling || !state.el?.isConnected) return;
    const cells = pendingCells();
    if (!cells.length) {
      updateStats();
      return;
    }
    state.polling = true;
    try {
      const visibleKeys = [];
      for (const card of state.visible) {
        if (card.classList.contains("hidden")) continue;
        card.querySelectorAll(".lib-cell[data-state=queued]").forEach(cell => visibleKeys.push(cell.dataset.key));
      }
      const keys = [...new Set(cells.map(cell => cell.dataset.key))].slice(0, 400);
      const params = new URLSearchParams({ keys: keys.join(","), visible: visibleKeys.slice(0, 60).join(",") });
      const payload = await fetch(state.api(`/api/kicad/library/status?${params}`)).then(r => r.json());
      for (const cell of cells) {
        const status = payload.states?.[cell.dataset.key];
        if (status) setCell(cell, status);
      }
      updateStats();
    } catch (err) {
      console.warn("library status poll failed", err);
    } finally {
      state.polling = false;
    }
    if (pendingCells().length) schedulePoll(POLL_MS);
  };

  // ---- modal viewer --------------------------------------------------------

  let modal;
  let modalListener;
  const detachListener = () => {
    if (modalListener) window.removeEventListener("message", modalListener);
    modalListener = undefined;
  };
  const closeModal = () => {
    detachListener();
    modal?.remove();
    modal = undefined;
  };

  const openModal = (part, kind) => {
    closeModal();
    modal = document.createElement("div");
    modal.className = "lib-modal";
    const tabs = [...KINDS, ["render", "Large render"]]
      .map(([id, label]) => {
        const enabled = id === "render" ? KINDS.some(([k]) => part.thumbs?.[k]?.state === "ready") : !!part.thumbs?.[id]?.key;
        return `<button data-view="${id}"${enabled ? "" : " disabled"}>${esc(label)}</button>`;
      })
      .join("");
    modal.innerHTML = `<div class="lib-dialog" role="dialog" aria-modal="true"><header><h3>${esc(part.title)}</h3><div class="lib-tabs">${tabs}</div><button class="lib-close">Close</button></header><div class="lib-stage"></div><footer>${esc([compactRefs(part.refs), (part.values || []).join(", "), part.symbol].filter(Boolean).join(" · "))}</footer></div>`;
    modal.addEventListener("click", event => {
      if (event.target === modal) closeModal();
    });
    modal.querySelector(".lib-close").onclick = closeModal;
    modal.querySelectorAll(".lib-tabs button").forEach(button => (button.onclick = () => show(part, button.dataset.view)));
    document.body.append(modal);
    show(part, kind);
  };

  const show = (part, view) => {
    if (!modal) return;
    modal.querySelectorAll(".lib-tabs button").forEach(button => button.classList.toggle("active", button.dataset.view === view));
    detachListener();
    const stage = modal.querySelector(".lib-stage");
    const footer = modal.querySelector("footer");
    if (view === "render") {
      const imgs = KINDS.map(([k]) => part.thumbs?.[k]).filter(t => t?.state === "ready" && t.url);
      stage.innerHTML = `<div style="position:absolute;inset:0;display:grid;grid-template-columns:repeat(${imgs.length || 1},1fr)">${imgs.map(t => `<div style="position:relative"><img src="${esc(t.url)}"></div>`).join("")}</div>`;
      return;
    }
    const thumb = part.thumbs?.[view];
    if (!thumb?.key) {
      stage.innerHTML = `<div class="lib-stage-msg">${esc(thumb?.message || "Nothing to show")}</div>`;
      return;
    }
    if (!thumb.viewer) {
      stage.innerHTML = thumb.state === "ready" ? `<img src="${esc(thumb.url)}">` : `<div class="lib-stage-msg">${esc(placeholderText(thumb))}</div>`;
      return;
    }
    const src = thumb.source ? `rendered from ${thumb.source} copy` : "";
    footer.dataset.base ||= footer.textContent;
    footer.textContent = [footer.dataset.base, src].filter(Boolean).join(" · ");
    stage.innerHTML = `<div class="lib-stage-msg">Loading viewer...</div><iframe src="/kicad-viewer/runtime.html"></iframe>`;
    const frame = stage.querySelector("iframe");
    const post = async () => {
      if (view === "model") {
        frame.contentWindow?.postMessage({ type: "kicad-pcb-snapshot", kind: "model", url: thumb.viewer, active: true }, location.origin);
        return;
      }
      const content = await fetch(thumb.viewer).then(r => {
        if (!r.ok) throw new Error(`HTTP ${r.status}`);
        return r.text();
      });
      const filename = view === "symbol" ? `${part.id}.kicad_sch` : `${part.id}.kicad_pcb`;
      frame.contentWindow?.postMessage(
        { type: "kicad-pcb-snapshot", kind: "native", context: view === "symbol" ? "schematic" : "pcb", revision: thumb.key, sources: [{ filename, content }], active: true },
        location.origin,
      );
    };
    const onMessage = event => {
      if (event.source !== frame.contentWindow || event.data?.type !== "kicad-pcb-runtime-ready") return;
      stage.querySelector(".lib-stage-msg")?.remove();
      post().catch(err => {
        stage.insertAdjacentHTML("beforeend", `<div class="lib-stage-msg">Viewer failed: ${esc(err.message)}</div>`);
      });
    };
    modalListener = onMessage;
    window.addEventListener("message", onMessage);
    frame.addEventListener("load", () => setTimeout(() => stage.querySelector(".lib-stage-msg")?.remove(), 4000), { once: true });
  };

  document.addEventListener("keydown", event => {
    if (event.key === "Escape") closeModal();
  });

  // ---- entry point -----------------------------------------------------------

  const signatureOf = data =>
    JSON.stringify([...(data.parts || []), ...(data.unused || [])].map(p => [p.id, p.badges?.length, ...KINDS.map(([k]) => p.thumbs?.[k]?.key)]));

  const load = async (el, api) => {
    state.el = el;
    if (api) state.api = api;
    if (!state.data) el.innerHTML = `<p class="muted">Building part inventory...</p>`;
    try {
      const response = await fetch(state.api("/api/kicad/library"));
      if (!response.ok) throw new Error(`HTTP ${response.status}: ${await response.text()}`);
      const data = await response.json();
      const signature = signatureOf(data);
      state.data = data;
      if (signature !== state.signature || !el.querySelector(".lib-grid")) {
        state.signature = signature;
        render();
      } else {
        updateStats();
      }
      schedulePoll(100);
    } catch (err) {
      el.innerHTML = `<p class="warning">Unable to load library inventory: ${esc(err.message)}</p>`;
    }
  };

  window.KicadLibrary = { load };
})();
