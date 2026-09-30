/* Waveform geometry is drawn here; VCD syntax and aliases are parsed by vcdvcd. */
"use strict";
const $ = (id) => document.getElementById(id);
const canvas = $("canvas"),
  ctx = canvas.getContext("2d");
const state = {
  info: null,
  run: null,
  meta: null,
  traces: [],
  selected: [],
  radices: {},
  focus: null,
  start: 0,
  end: 1,
  a: 0,
  b: null,
  epoch: 0,
  waveEpoch: 0,
  pointers: new Map(),
  gesture: null,
  note: null,
};
const rowHeight = 32,
  ruler = 28;
let labelWidth = 210,
  width = 900,
  height = 300,
  socket;
window.lucide?.createIcons();

function error(message = "") {
  $("error").hidden = !message;
  $("error").textContent = message;
}
async function api(path, data) {
  const response = await fetch(
    path,
    data === undefined
      ? {}
      : {
          method: "POST",
          headers: {
            "Content-Type": "application/json",
            "X-Verilog-Request": "1",
          },
          body: JSON.stringify(data),
        },
  );
  const result = await response.json();
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
  if (items.some(([value]) => value === current)) select.value = current;
}
function key() {
  return `verilog-view:${state.info?.project}:${state.run?.test}`;
}
function save() {
  if (!state.run) return;
  try {
    localStorage.setItem(
      key(),
      JSON.stringify({
        selected: state.selected,
        radices: state.radices,
        start: state.start,
        end: state.end,
        a: state.a,
        b: state.b,
      }),
    );
  } catch {}
}
function restore() {
  try {
    return JSON.parse(localStorage.getItem(key())) || {};
  } catch {
    return {};
  }
}
function clampRange(start, end) {
  const total = Math.max(1, state.meta?.end || 1);
  const span = Math.max(1, Math.min(total, end - start));
  state.start = Math.max(0, Math.min(total - span, start));
  state.end = state.start + span;
}
function unit() {
  return state.meta?.timescale || "ticks";
}
function time(t) {
  const seconds = Number(state.meta?.timescale || 1),
    span = (state.end - state.start) * seconds;
  const [scale, suffix] =
    span < 1e-9
      ? [1e-12, "ps"]
      : span < 1e-6
        ? [1e-9, "ns"]
        : span < 1e-3
          ? [1e-6, "us"]
          : span < 1
            ? [1e-3, "ms"]
            : [1, "s"];
  return `${Number(((t * seconds) / scale).toPrecision(5))} ${suffix}`;
}
function screenX(t) {
  return (
    labelWidth +
    ((t - state.start) / (state.end - state.start)) * (width - labelWidth)
  );
}
function eventTime(x) {
  return Math.round(
    Math.max(
      0,
      Math.min(
        state.meta?.end || 1,
        state.start +
          ((x - labelWidth) / (width - labelWidth)) * (state.end - state.start),
      ),
    ),
  );
}
function valueAt(events, t) {
  let lo = 0,
    hi = events.length;
  while (lo < hi) {
    const m = (lo + hi) >> 1;
    if (events[m][0] <= t) lo = m + 1;
    else hi = m;
  }
  return lo ? events[lo - 1][1] : "x";
}
function format(value, trace) {
  if (trace.type === "real" || /[xz]/i.test(value)) return value;
  try {
    const n = BigInt("0b" + value),
      radix = state.radices[trace.name] || "hex";
    if (radix === "bin") return value.padStart(trace.width, "0");
    if (radix === "hex")
      return "0x" + n.toString(16).padStart(Math.ceil(trace.width / 4), "0");
    if (radix === "signed" && n & (1n << BigInt(trace.width - 1)))
      return (n - (1n << BigInt(trace.width))).toString();
    return n.toString();
  } catch {
    return value;
  }
}

async function refresh() {
  const info = await api("/api/state");
  state.info = info;
  error(
    info.error ||
      (!info.ok
        ? "Icarus Verilog is unavailable. Run plugin doctor to inspect dependencies."
        : ""),
  );
  options(
    $("test"),
    info.tests.map((t) => [t.id, t.top]),
    $("test").value,
  );
  $("run").disabled = !!info.active || !info.tests.length || !info.ok;
  $("stop").disabled = !info.active;
  if (socket?.readyState === WebSocket.OPEN)
    $("live").textContent = info.active ? "Simulating" : "Connected";
  const previous = $("runs").value;
  options(
    $("runs"),
    info.runs.map((r) => [
      r.id,
      `${r.top} - ${r.status} - ${r.started.slice(11, 19)}`,
    ]),
    previous,
  );
  // Follow a run started here, but preserve an explicitly selected historical run.
  if (state.follow && info.runs.length && info.runs[0].id !== state.follow) {
    $("runs").value = info.runs[0].id;
    state.follow = null;
  }
  const next = info.runs.find((r) => r.id === $("runs").value);
  if (next && (next.id !== state.run?.id || next.status !== state.run?.status))
    await loadRun(next);
}
async function loadRun(run) {
  state.run = run;
  const epoch = ++state.epoch;
  $("result").textContent = run.status;
  $("result").className = run.status === "passed" ? "passed" : "failed";
  const log = await fetch(`/api/log/${run.id}`).then((r) => r.text());
  if (epoch !== state.epoch) return;
  $("log").textContent = (run.message ? run.message + "\n" : "") + log;
  $("logs").open = !["passed", "running"].includes(run.status);
  options(
    $("files"),
    run.waves.map((f) => [f, f]),
    $("files").value,
  );
  await loadWave();
}
async function loadWave() {
  const epoch = ++state.waveEpoch;
  ++state.fetchEpoch;
  const file = $("files").value;
  state.meta = null;
  state.traces = [];
  $("download").hidden = !file;
  if (!file) {
    $("hierarchy").replaceChildren();
    draw();
    return;
  }
  $("download").href =
    `/api/download/${state.run.id}?file=${encodeURIComponent(file)}`;
  $("download").download = file;
  const meta = await api(
    `/api/wave?${new URLSearchParams({ run: state.run.id, file })}`,
  );
  if (epoch !== state.waveEpoch) return;
  state.meta = meta;
  const old = restore(),
    names = new Set(meta.signals.map((s) => s.name));
  state.selected = Array.isArray(old.selected)
    ? old.selected.filter((n) => names.has(n)).slice(0, 64)
    : meta.signals.slice(0, 8).map((s) => s.name);
  state.radices = old.radices || {};
  state.focus = state.selected[0];
  state.a = Number.isFinite(old.a) ? Math.min(meta.end, old.a) : 0;
  state.b = Number.isFinite(old.b) ? Math.min(meta.end, old.b) : null;
  clampRange(
    Number.isFinite(old.start) ? old.start : 0,
    Number.isFinite(old.end) ? old.end : meta.end,
  );
  hierarchy();
  await fetchTraces();
}
function hierarchy() {
  const filter = $("search").value.toLowerCase(),
    groups = new Map();
  for (const sig of state.meta?.signals || []) {
    if (!sig.name.toLowerCase().includes(filter)) continue;
    const split = sig.name.lastIndexOf("."),
      scope = sig.name.slice(0, split) || "Top";
    if (!groups.has(scope)) groups.set(scope, []);
    groups.get(scope).push(sig);
  }
  const fragment = document.createDocumentFragment();
  for (const [scope, signals] of groups) {
    const details = document.createElement("details"),
      summary = document.createElement("summary");
    details.open = true;
    summary.textContent = scope;
    details.append(summary);
    for (const signal of signals) {
      const label = document.createElement("label"),
        check = document.createElement("input"),
        text = document.createElement("span");
      check.type = "checkbox";
      check.checked = state.selected.includes(signal.name);
      text.textContent = signal.name.slice(signal.name.lastIndexOf(".") + 1);
      label.title = signal.name;
      check.onchange = () => {
        if (check.checked && state.selected.length >= 64) {
          check.checked = false;
          error("Select at most 64 signals");
          return;
        }
        state.selected = check.checked
          ? [...state.selected, signal.name]
          : state.selected.filter((n) => n !== signal.name);
        state.focus = signal.name;
        save();
        fetchTraces().catch((e) => error(e.message));
      };
      label.append(check, text);
      details.append(label);
    }
    fragment.append(details);
  }
  $("hierarchy").replaceChildren(fragment);
}
state.fetchEpoch = 0;
async function fetchTraces() {
  const epoch = ++state.fetchEpoch;
  if (!state.meta || !state.selected.length) {
    state.traces = [];
    draw();
    return;
  }
  const params = new URLSearchParams({
    run: state.run.id,
    file: $("files").value,
    start: Math.floor(state.start),
    end: Math.ceil(state.end),
  });
  state.selected.forEach((name) => params.append("signal", name));
  try {
    const data = await api(`/api/wave?${params}`);
    if (epoch !== state.fetchEpoch) return;
    state.traces = data.traces;
    error();
    draw();
    save();
  } catch (e) {
    if (epoch !== state.fetchEpoch) return;
    state.traces = [];
    draw();
    error(e.message);
  }
}
function draw() {
  width = Math.max(160, $("waves").clientWidth);
  labelWidth = Math.min(210, Math.max(85, width * 0.32));
  height = Math.max(
    $("waves").clientHeight,
    ruler + state.traces.length * rowHeight,
  );
  const scale = devicePixelRatio || 1;
  canvas.width = Math.round(width * scale);
  canvas.height = Math.round(height * scale);
  canvas.style.width = width + "px";
  canvas.style.height = height + "px";
  ctx.setTransform(scale, 0, 0, scale, 0, 0);
  ctx.fillStyle = "#121519";
  ctx.fillRect(0, 0, width, height);
  ctx.font = "11px monospace";
  $("empty").hidden = !!state.traces.length;
  $("empty").textContent = state.meta
    ? "Select signals to inspect."
    : state.run?.status === "running"
      ? "Simulation running..."
      : "No waveform in this run.";
  $("range").textContent = state.meta
    ? `${time(state.start)} - ${time(state.end)} | tick = ${unit()} s`
    : "No waveform";
  $("markers").textContent = state.meta
    ? `A ${time(state.a)}${state.b === null ? "" : ` | B ${time(state.b)} | delta ${time(Math.abs(state.a - state.b))}`}`
    : "";
  ctx.strokeStyle = "#2d333b";
  ctx.fillStyle = "#a9b2bd";
  const divisions = Math.max(1, Math.floor((width - labelWidth) / 95));
  for (let i = 0; i <= divisions; i++) {
    const t = state.start + ((state.end - state.start) * i) / divisions,
      x = screenX(t);
    ctx.beginPath();
    ctx.moveTo(x, 0);
    ctx.lineTo(x, height);
    ctx.stroke();
    if (i < divisions) ctx.fillText(time(t), x + 4, 17);
  }
  state.traces.forEach((trace, i) => {
    const y = ruler + i * rowHeight;
    ctx.fillStyle = i % 2 ? "#ffffff04" : "#ffffff00";
    ctx.fillRect(0, y, width, rowHeight);
    ctx.save();
    ctx.beginPath();
    ctx.rect(labelWidth, y, width - labelWidth, rowHeight);
    ctx.clip();
    const events = trace.events;
    events.forEach(([t, value], j) => {
      const x = Math.max(labelWidth, screenX(t)),
        end = Math.min(width, screenX(events[j + 1]?.[0] ?? state.end));
      if (end < x) return;
      const unknown = /x/i.test(value),
        highz = /z/i.test(value);
      ctx.strokeStyle = unknown ? "#f7768e" : highz ? "#e0af68" : "#9ece6a";
      ctx.fillStyle = ctx.strokeStyle;
      const upper = y + 7,
        lower = y + 25;
      ctx.beginPath();
      if (trace.width === 1 && trace.type !== "real" && !unknown && !highz) {
        const level = value === "1" ? upper : lower;
        ctx.moveTo(x, level);
        ctx.lineTo(end, level);
        if (j && t >= state.start) {
          ctx.moveTo(x, upper);
          ctx.lineTo(x, lower);
        }
        ctx.stroke();
      } else {
        ctx.moveTo(x, y + 16);
        ctx.lineTo(Math.min(end, x + 3), upper);
        ctx.lineTo(Math.max(x, end - 3), upper);
        ctx.lineTo(end, y + 16);
        ctx.lineTo(Math.max(x, end - 3), lower);
        ctx.lineTo(Math.min(end, x + 3), lower);
        ctx.closePath();
        ctx.stroke();
        const text = format(value, trace);
        if (end - x > ctx.measureText(text).width + 12)
          ctx.fillText(text, x + 6, y + 20);
      }
    });
    ctx.restore();
    ctx.fillStyle = trace.name === state.focus ? "#30383e" : "#1e2328";
    ctx.fillRect(0, y, labelWidth, rowHeight - 1);
    ctx.save();
    ctx.beginPath();
    ctx.rect(4, y, labelWidth - 8, rowHeight);
    ctx.clip();
    ctx.fillStyle = "#dce0e7";
    let label = trace.name;
    if (ctx.measureText(label).width > labelWidth - 12) {
      label = label.slice(label.lastIndexOf(".") + 1);
      while (label.length && ctx.measureText("..." + label).width > labelWidth - 12) {
        label = label.slice(1);
      }
      label = "..." + label;
    }
    ctx.fillText(label, 6, y + 12);
    ctx.fillStyle = "#7dcfff";
    ctx.fillText(format(valueAt(events, state.a), trace), 6, y + 26);
    ctx.restore();
  });
  for (const [t, color, label] of [
    [state.a, "#f7768e", "A"],
    [state.b, "#e0af68", "B"],
  ]) {
    if (t === null || t < state.start || t > state.end) continue;
    const x = screenX(t);
    ctx.strokeStyle = color;
    ctx.beginPath();
    ctx.moveTo(x, ruler);
    ctx.lineTo(x, height);
    ctx.stroke();
    ctx.fillStyle = color;
    ctx.fillText(label, x + 3, ruler - 3);
  }
}
function zoom(factor, center = (state.start + state.end) / 2) {
  const span = (state.end - state.start) * factor,
    ratio = (center - state.start) / (state.end - state.start);
  clampRange(center - span * ratio, center + span * (1 - ratio));
  fetchTraces();
}
function transition(direction) {
  const trace =
    state.traces.find((t) => t.name === state.focus) || state.traces[0];
  if (!trace) return;
  const times = trace.events.map((e) => e[0]);
  const next =
    direction > 0
      ? times.find((t) => t > state.a)
      : times.findLast((t) => t < state.a);
  if (next !== undefined) {
    state.a = next;
    draw();
    save();
  } else error("No further transition in this window; pan or zoom out.");
}
function local(event) {
  const box = canvas.getBoundingClientRect();
  return { x: event.clientX - box.left, y: event.clientY - box.top };
}
canvas.addEventListener("pointerdown", (e) => {
  if (!state.meta) return;
  e.preventDefault();
  canvas.setPointerCapture(e.pointerId);
  const p = local(e);
  state.pointers.set(e.pointerId, p);
  if (p.x < labelWidth && state.pointers.size === 1) {
    state.focus = state.traces[Math.floor((p.y - ruler) / rowHeight)]?.name;
    $("radix").value = state.radices[state.focus] || "hex";
    draw();
    return;
  }
  resetGesture();
  if (state.pointers.size === 1 && $("mode").value !== "pan") {
    state[$("mode").value === "baseline" ? "b" : "a"] = eventTime(p.x);
    draw();
  }
});
function resetGesture() {
  const points = [...state.pointers.values()];
  state.gesture = points.length
    ? {
        start: state.start,
        end: state.end,
        x: points.reduce((s, p) => s + p.x, 0) / points.length,
        distance:
          points.length > 1
            ? Math.hypot(points[0].x - points[1].x, points[0].y - points[1].y)
            : 0,
      }
    : null;
}
let traceTimer;
canvas.addEventListener("pointermove", (e) => {
  if (!state.pointers.has(e.pointerId) || !state.gesture) return;
  const p = local(e);
  state.pointers.set(e.pointerId, p);
  const points = [...state.pointers.values()],
    g = state.gesture;
  if (points.length > 1 || $("mode").value === "pan") {
    const x = points.reduce((s, p) => s + p.x, 0) / points.length;
    const distance =
      points.length > 1
        ? Math.hypot(points[0].x - points[1].x, points[0].y - points[1].y)
        : 0;
    const span =
      (g.end - g.start) * (g.distance && distance ? g.distance / distance : 1);
    const anchor =
      g.start + ((g.x - labelWidth) / (width - labelWidth)) * (g.end - g.start);
    const start = anchor - ((x - labelWidth) / (width - labelWidth)) * span;
    clampRange(start, start + span);
    draw();
    clearTimeout(traceTimer);
    traceTimer = setTimeout(fetchTraces, 100);
  } else {
    state[$("mode").value === "baseline" ? "b" : "a"] = eventTime(p.x);
    draw();
  }
});
function endPointer(e) {
  state.pointers.delete(e.pointerId);
  resetGesture();
  save();
  if (!state.pointers.size) fetchTraces();
}
for (const name of ["pointerup", "pointercancel", "lostpointercapture"])
  canvas.addEventListener(name, endPointer);
canvas.addEventListener(
  "wheel",
  (e) => {
    if (!state.meta) return;
    e.preventDefault();
    if (e.ctrlKey || e.metaKey)
      zoom(Math.exp(e.deltaY * 0.005), eventTime(local(e).x));
    else {
      const delta =
        ((e.deltaX || e.deltaY) / (width - labelWidth)) *
        (state.end - state.start);
      clampRange(state.start + delta, state.end + delta);
      fetchTraces();
    }
  },
  { passive: false },
);
window.addEventListener("blur", () => {
  state.pointers.clear();
  state.gesture = null;
});
$("zoom-in").onclick = () => zoom(0.5);
$("zoom-out").onclick = () => zoom(2);
$("fit").onclick = () => {
  clampRange(0, state.meta?.end || 1);
  fetchTraces();
};
$("previous").onclick = () => transition(-1);
$("next").onclick = () => transition(1);
$("radix").onchange = () => {
  if (state.focus) state.radices[state.focus] = $("radix").value;
  draw();
  save();
};
$("search").oninput = hierarchy;
$("runs").onchange = () =>
  loadRun(state.info.runs.find((r) => r.id === $("runs").value)).catch((e) =>
    error(e.message),
  );
$("files").onchange = () => loadWave().catch((e) => error(e.message));
$("run").onclick = async () => {
  try {
    $("run").disabled = true;
    state.follow = state.info.runs[0]?.id || "none";
    await api("/api/run", { test: $("test").value });
    await refresh();
  } catch (e) {
    error(e.message);
    $("run").disabled = false;
  }
};
$("stop").onclick = () =>
  api("/api/cancel", {})
    .then(refresh)
    .catch((e) => error(e.message));
new ResizeObserver(draw).observe($("waves"));
$("sidebar").onclick = () => {
  document.body.classList.toggle(
    matchMedia("(max-width:700px)").matches ? "sidebar-open" : "sidebar-hidden",
  );
  draw();
};

$("annotate").onclick = () => {
  if (!state.meta || !state.traces.length) {
    error("Select a completed waveform first");
    return;
  }
  const interval =
    state.b === null
      ? [state.start, state.end]
      : [Math.min(state.a, state.b), Math.max(state.a, state.b)];
  const capture = document.createElement("canvas");
  capture.width = Math.min(900, canvas.width);
  capture.height = Math.round(
    (Math.min(canvas.height, 900) * capture.width) / canvas.width,
  );
  capture
    .getContext("2d")
    .drawImage(
      canvas,
      0,
      0,
      canvas.width,
      Math.min(canvas.height, 900),
      0,
      0,
      capture.width,
      capture.height,
    );
  state.note = {
    title: `${state.run.top}: ${time(interval[0])} - ${time(interval[1])}`,
    context: {
      plugin: "verilog",
      project: state.info.project,
      run: state.run.id,
      test: state.run.test,
      revision: state.run.revision,
      waveform: $("files").value,
      signals: [...state.selected],
      interval,
      timescale: unit(),
      cursorA: state.a,
      cursorB: state.b,
    },
    image: { dataUrl: capture.toDataURL("image/png") },
  };
  $("capture").src = state.note.image.dataUrl;
  $("note-context").textContent = state.note.title;
  $("note-status").textContent = "";
  $("note").showModal();
};
$("send").onclick = async () => {
  if (!$("body").value.trim()) {
    $("note-status").textContent = "Add a note before queuing.";
    return;
  }
  $("send").disabled = true;
  try {
    const response = await fetch("/__portal/edit-stack", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        source: {
          plugin: "verilog",
          project: state.info.project,
          session: state.info.session,
          page: location.href,
        },
        items: [{ ...state.note, body: $("body").value }],
      }),
    });
    if (!response.ok) throw new Error(await response.text());
    $("body").value = "";
    $("note").close();
  } catch (e) {
    $("note-status").textContent =
      `Not queued: ${e.message}. Open this pane through Agent Portal to send feedback.`;
  } finally {
    $("send").disabled = false;
  }
};
const Recognition = window.SpeechRecognition || window.webkitSpeechRecognition;
let recognition;
$("record").disabled = !Recognition;
$("record").title = Recognition
  ? "Record voice note"
  : "Voice recognition unavailable in this browser";
$("record").onclick = () => {
  if (recognition) {
    recognition.stop();
    return;
  }
  recognition = new Recognition();
  recognition.continuous = true;
  recognition.onresult = (e) => {
    for (let i = e.resultIndex; i < e.results.length; i++)
      if (e.results[i].isFinal)
        $("body").value +=
          ($("body").value ? " " : "") + e.results[i][0].transcript;
  };
  recognition.onerror = (e) => ($("note-status").textContent = e.error);
  recognition.onend = () => {
    recognition = null;
    $("record").classList.remove("failed");
  };
  recognition.start();
  $("record").classList.add("failed");
};
$("note").addEventListener("close", () => recognition?.stop());
function connect() {
  socket = new WebSocket(
    `${location.protocol === "https:" ? "wss:" : "ws:"}//${location.host}/ws`,
  );
  socket.onopen = () => refresh().catch((e) => error(e.message));
  socket.onmessage = () => refresh().catch((e) => error(e.message));
  socket.onclose = () => {
    $("live").textContent = "Disconnected";
    setTimeout(connect, 1500);
  };
  socket.onerror = () => socket.close();
}
connect();
