/* Pane state and plotting; model import, SVG and simulation come from the unlinked CLI. */
"use strict";
const $ = (id) => document.getElementById(id);
const PALETTE = ["#7aa2f7", "#9ece6a", "#f7768e", "#e0af68", "#bb9af7", "#7dcfff", "#ff9e64", "#73daca"];
const SOLVER_MAP = { ode1: "euler", ode4: "rk4", ode45: "rk45" };
const state = { info: null, model: null, systems: [], names: {}, trace: null, selected: new Set(), runs: [] };

function error(message = "") {
  $("error").hidden = !message;
  $("error").textContent = message;
}
function empty(html) {
  $("empty").hidden = !html;
  if (html) $("empty-text").innerHTML = html;
}
async function api(path, data) {
  const response = await fetch(
    path,
    data === undefined
      ? {}
      : {
          method: "POST",
          headers: { "Content-Type": "application/json", "X-Unlinked-Request": "1" },
          body: JSON.stringify(data),
        },
  );
  const type = response.headers.get("Content-Type") || "";
  const result = type.includes("json") ? await response.json() : await response.text();
  if (!response.ok) throw new Error(result.error || `HTTP ${response.status}`);
  return result;
}
function options(select, items, current) {
  select.replaceChildren(
    ...items.map(([value, text]) => {
      const option = document.createElement("option");
      option.value = value;
      option.textContent = text;
      return option;
    }),
  );
  if (items.some(([v]) => v === current)) select.value = current;
}
const key = (part) => `unlinked:${state.info?.project || ""}:${part}`;

async function refresh() {
  state.info = await api("/api/state");
  state.runs = state.info.runs;
  const remembered = localStorage.getItem(key("model"));
  options($("model"), state.info.models.map((m) => [m, m]), remembered);
  error(state.info.error || "");
  $("status").textContent = state.info.ok ? `unlinked ${state.info.revision.slice(0, 10)}` : "";
  if (!state.info.ok) {
    empty("Unlinked is not installed. Run <code>bin/unlinked setup</code> (needs Cargo) and reload.");
  } else if (!state.info.models.length) {
    empty("No <code>.slx</code> or <code>.mdl</code> models found under this project.");
  } else {
    empty("");
    await selectModel($("model").value);
  }
}

async function selectModel(model) {
  state.model = model;
  localStorage.setItem(key("model"), model);
  state.trace = null;
  state.selected.clear();
  $("info-text").textContent = "";
  $("svg-host").replaceChildren();
  error();
  try {
    const info = await api(`/api/info?file=${encodeURIComponent(model)}`);
    $("info-text").textContent = JSON.stringify(info, null, 2);
    state.systems = info.systems.map((s) => s.path);
    options($("system"), state.systems.map((p) => [p, p]), state.systems[0]);
    $("system").hidden = state.systems.length < 2 || $("view").value !== "diagram";
    const config = info.imported_config || {};
    $("stop").value = Number(config.stop_time) || 10;
    $("step").value = Number(config.fixed_step) || 0.01;
    $("solver").value = SOLVER_MAP[config.solver] || "rk4";
    $("vars").value = (info.workspace_variables || []).length ? "" : $("vars").value;
    await Promise.all([loadDiagram(), showRuns()]);
  } catch (e) {
    error(e.message);
  }
}

async function loadDiagram() {
  const system = $("system").value || "";
  const svg = await api(`/api/render?file=${encodeURIComponent(state.model)}&system=${encodeURIComponent(system)}`);
  $("svg-host").innerHTML = svg;
  const inports = [];
  for (const block of $("svg-host").querySelectorAll("[data-sid][data-name]")) {
    state.names[block.dataset.sid] = block.dataset.name;
    if (block.dataset.type === "Inport") inports.push(block.dataset.sid);
  }
  // Root Inports need a value to simulate; offer zero for each, editable.
  if (!$("inputs").value.trim()) $("inputs").value = inports.map((id) => `${id}=0`).join("\n");
}

function showView() {
  const view = $("view").value;
  $("diagram").hidden = view !== "diagram";
  $("simulate").hidden = view !== "simulate";
  $("system").hidden = view !== "diagram" || state.systems.length < 2;
}

async function showRuns() {
  const runs = state.runs.filter((r) => r.model === state.model);
  options(
    $("runs"),
    [["", runs.length ? "Previous runs…" : "No runs yet"]].concat(
      runs.map((r) => [r.id, `${r.started.slice(11, 19)} ${r.status} ${r.settings.solver} step ${r.settings.step}`]),
    ),
    "",
  );
}

async function runSimulation(event) {
  event.preventDefault();
  $("run").disabled = true;
  $("run-log").hidden = true;
  error();
  $("status").textContent = "Simulating…";
  try {
    const record = await api("/api/sim", {
      file: state.model,
      start: Number($("start").value),
      stop: Number($("stop").value),
      step: Number($("step").value),
      solver: $("solver").value,
      vars: $("vars").value.split("\n").map((v) => v.trim()).filter(Boolean),
      inputs: $("inputs").value.split("\n").map((v) => v.trim()).filter(Boolean),
    });
    state.runs.unshift(record);
    await showRuns();
    $("runs").value = record.id;
    await showRun(record);
  } catch (e) {
    error(e.message);
  } finally {
    $("run").disabled = false;
    $("status").textContent = state.model;
  }
}

async function showRun(record) {
  if (record.status !== "completed") {
    const log = await api(`/api/log/${record.id}`);
    $("run-log").textContent = `${record.status}: ${record.message}\n${log}`.trim();
    $("run-log").hidden = false;
    state.trace = null;
    draw();
    return;
  }
  $("run-log").hidden = true;
  const data = await api(`/api/trace/${record.id}`);
  state.trace = data.trace;
  const ids = Object.keys(state.trace.signals);
  if (![...state.selected].some((id) => ids.includes(id))) {
    state.selected = new Set(ids.slice(0, 4));
  }
  draw();
}

function legend(ids) {
  $("legend").replaceChildren(
    ...ids.map((id, index) => {
      const item = document.createElement("li");
      const box = document.createElement("input");
      box.type = "checkbox";
      box.checked = state.selected.has(id);
      box.addEventListener("change", () => {
        box.checked ? state.selected.add(id) : state.selected.delete(id);
        draw();
      });
      const swatch = document.createElement("i");
      swatch.style.background = PALETTE[index % PALETTE.length];
      const label = document.createElement("span");
      const [sid, ...rest] = id.split("/");
      const name = state.names[sid];
      label.textContent = name ? `${[name, ...rest].join("/")} (${id})` : id;
      item.append(box, swatch, label);
      return item;
    }),
  );
}

function draw() {
  const canvas = $("plot"),
    ctx = canvas.getContext("2d");
  const rect = canvas.getBoundingClientRect();
  canvas.width = Math.max(300, rect.width * devicePixelRatio);
  canvas.height = Math.max(200, rect.height * devicePixelRatio);
  ctx.setTransform(devicePixelRatio, 0, 0, devicePixelRatio, 0, 0);
  const width = canvas.width / devicePixelRatio,
    height = canvas.height / devicePixelRatio;
  ctx.fillStyle = "#1a1b26";
  ctx.fillRect(0, 0, width, height);
  if (!state.trace) {
    $("legend").replaceChildren();
    return;
  }
  const ids = Object.keys(state.trace.signals);
  legend(ids);
  const time = state.trace.time;
  const shown = ids.filter((id) => state.selected.has(id));
  let low = Infinity,
    high = -Infinity;
  for (const id of shown) for (const v of state.trace.signals[id]) if (Number.isFinite(v)) (low = Math.min(low, v)), (high = Math.max(high, v));
  if (!shown.length || !Number.isFinite(low)) return;
  if (high === low) (low -= 1), (high += 1);
  const pad = { left: 56, right: 12, top: 10, bottom: 26 };
  const x = (t) => pad.left + ((t - time[0]) / (time.at(-1) - time[0] || 1)) * (width - pad.left - pad.right);
  const y = (v) => pad.top + (1 - (v - low) / (high - low)) * (height - pad.top - pad.bottom);
  ctx.strokeStyle = "#565f89";
  ctx.fillStyle = "#c0caf5";
  ctx.font = "11px system-ui";
  ctx.lineWidth = 1;
  for (let i = 0; i <= 4; i++) {
    const v = low + ((high - low) * i) / 4;
    ctx.beginPath();
    ctx.moveTo(pad.left, y(v));
    ctx.lineTo(width - pad.right, y(v));
    ctx.stroke();
    ctx.textAlign = "right";
    ctx.fillText(v.toPrecision(3), pad.left - 6, y(v) + 4);
    const t = time[0] + ((time.at(-1) - time[0]) * i) / 4;
    ctx.textAlign = "center";
    ctx.fillText(t.toPrecision(3), x(t), height - 8);
  }
  ctx.lineWidth = 1.5;
  for (const id of shown) {
    ctx.strokeStyle = PALETTE[ids.indexOf(id) % PALETTE.length];
    ctx.beginPath();
    const samples = state.trace.signals[id];
    const stride = Math.max(1, Math.floor(samples.length / (width * 2)));
    for (let i = 0; i < samples.length; i += stride) {
      i === 0 ? ctx.moveTo(x(time[i]), y(samples[i])) : ctx.lineTo(x(time[i]), y(samples[i]));
    }
    ctx.stroke();
  }
}

$("model").addEventListener("change", () => selectModel($("model").value));
$("view").addEventListener("change", () => {
  showView();
  draw();
});
$("system").addEventListener("change", () => loadDiagram().catch((e) => error(e.message)));
$("sim-form").addEventListener("submit", runSimulation);
$("runs").addEventListener("change", () => {
  const record = state.runs.find((r) => r.id === $("runs").value);
  if (record) showRun(record).catch((e) => error(e.message));
});
addEventListener("resize", draw);
showView();
refresh().catch((e) => error(e.message));
