/* yapCAD workbench pane. Geometry arrives as view.json triangle soups (one
   per part) or 2D polylines; builds run server-side in retained run dirs. */
import * as THREE from "three";
import { OrbitControls } from "three/addons/controls/OrbitControls.js";

const $ = (id) => document.getElementById(id);
const state = {
  info: null,
  file: null,
  listing: null,
  run: null,
  view: null,
  follow: true,
  pins: [],
  measure: [],
  epoch: 0,
  reference: null,
  note: null,
};
let socket;
window.workbench = state; // inspection hook for tests and debugging
window.lucide?.createIcons();

// ------------------------------------------------------------------ basics

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
            "X-Yapcad-Request": "1",
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
function element(tag, props = {}, ...children) {
  const node = Object.assign(document.createElement(tag), props);
  node.append(...children.filter((c) => c !== null && c !== undefined));
  return node;
}
function storeKey(suffix) {
  return `yapcad:${state.info?.project}:${suffix}`;
}
function load(suffix, fallback) {
  try {
    return JSON.parse(localStorage.getItem(storeKey(suffix))) ?? fallback;
  } catch {
    return fallback;
  }
}
function store(suffix, value) {
  try {
    localStorage.setItem(storeKey(suffix), JSON.stringify(value));
  } catch {}
}
const fmt = (v, digits = 4) =>
  typeof v === "number" ? Number(v.toPrecision(digits)).toString() : String(v);

// ------------------------------------------------------------------- scene

const canvas = $("canvas");
const renderer = new THREE.WebGLRenderer({
  canvas,
  antialias: true,
  preserveDrawingBuffer: true,
});
renderer.setPixelRatio(window.devicePixelRatio);
renderer.localClippingEnabled = true;
const scene = new THREE.Scene();
scene.background = new THREE.Color(0x1a1b26);
THREE.Object3D.DEFAULT_UP.set(0, 0, 1);
const perspective = new THREE.PerspectiveCamera(40, 1, 0.01, 1e7);
const orthographic = new THREE.OrthographicCamera(-1, 1, 1, -1, -1e7, 1e7);
let camera = perspective;
for (const c of [perspective, orthographic]) c.up.set(0, 0, 1);
const controls = new OrbitControls(camera, canvas);
controls.enableDamping = true;
controls.dampingFactor = 0.15;
scene.add(new THREE.HemisphereLight(0xdde6ff, 0x30303a, 0.7));
const key = new THREE.DirectionalLight(0xffffff, 0.9);
const fill = new THREE.DirectionalLight(0xb0c4ff, 0.35);
key.position.set(0.4, 0.3, 1);
fill.position.set(-0.6, -0.4, -0.2);
perspective.add(key, fill);
orthographic.add(key.clone(), fill.clone());
scene.add(perspective, orthographic);
const model = new THREE.Group();
const marks = new THREE.Group();
scene.add(model, marks);
let grid = null;
const clipPlane = new THREE.Plane(new THREE.Vector3(1, 0, 0), 0);
const bounds = new THREE.Box3();
window.scene3d = {
  scene,
  model,
  marks,
  controls,
  get camera() {
    return camera;
  },
  pick: (e) => pick(e),
};
const raycaster = new THREE.Raycaster();
raycaster.params.Line.threshold = 0.5;

function resize() {
  const { clientWidth: w, clientHeight: h } = $("viewport");
  if (!w || !h) return;
  renderer.setSize(w, h, false);
  perspective.aspect = w / h;
  perspective.updateProjectionMatrix();
  const span = (orthographic.top - orthographic.bottom) / 2;
  orthographic.left = (-span * w) / h;
  orthographic.right = (span * w) / h;
  orthographic.updateProjectionMatrix();
}
new ResizeObserver(resize).observe($("viewport"));

function animate() {
  controls.update();
  renderer.render(scene, camera);
  placeLabels();
  requestAnimationFrame(animate);
}

const DIRECTIONS = {
  iso: [1, -1, 0.8],
  front: [0, -1, 0],
  back: [0, 1, 0],
  top: [0, 0, 1],
  bottom: [0, 0, -1],
  right: [1, 0, 0],
  left: [-1, 0, 0],
};
function fit(direction = $("view").value) {
  if (bounds.isEmpty()) return;
  const center = bounds.getCenter(new THREE.Vector3());
  const radius = Math.max(
    bounds.getSize(new THREE.Vector3()).length() / 2,
    1e-6,
  );
  const dir = new THREE.Vector3(...DIRECTIONS[direction]).normalize();
  const up =
    Math.abs(dir.z) > 0.99
      ? new THREE.Vector3(0, 1, 0)
      : new THREE.Vector3(0, 0, 1);
  const distance =
    radius / Math.sin(THREE.MathUtils.degToRad(perspective.fov / 2));
  for (const c of [perspective, orthographic]) {
    c.up.copy(up);
    c.position.copy(center).addScaledVector(dir, distance * 1.1);
    c.near = distance / 1000;
    c.far = distance * 100;
  }
  const aspect = perspective.aspect || 1;
  orthographic.top = radius * 1.15;
  orthographic.bottom = -radius * 1.15;
  orthographic.left = -radius * 1.15 * aspect;
  orthographic.right = radius * 1.15 * aspect;
  orthographic.near = -distance * 100;
  orthographic.zoom = 1;
  perspective.updateProjectionMatrix();
  orthographic.updateProjectionMatrix();
  controls.target.copy(center);
  controls.update();
}
function setCamera(ortho) {
  const next = ortho ? orthographic : perspective;
  if (next === camera) return;
  next.position.copy(camera.position);
  next.up.copy(camera.up);
  camera = next;
  controls.object = camera;
  controls.update();
}

function disposeGroup(group) {
  for (const child of [...group.children]) {
    child.traverse((o) => {
      o.geometry?.dispose();
      if (o.material) [].concat(o.material).forEach((m) => m.dispose());
    });
    group.remove(child);
  }
}
function decode(base64) {
  const bytes = Uint8Array.from(atob(base64), (c) => c.charCodeAt(0));
  return new Float32Array(bytes.buffer);
}
function materials() {
  const planes = $("clip-axis").value ? [clipPlane] : [];
  model.traverse((o) => {
    if (!o.material) return;
    o.material.clippingPlanes = planes;
    if (o.isMesh) o.material.wireframe = $("wire").checked;
  });
  model.traverse((o) => {
    if (o.userData.edges) o.visible = $("edges").checked && o.parent.visible;
  });
}

function showView(view) {
  disposeGroup(model);
  // Pins survive rebuilds of the same command so live edits keep the review context.
  const key = `${state.run?.file}:${state.run?.command}`;
  if (key !== state.marksKey) {
    state.pins = [];
    state.measure = [];
  }
  state.marksKey = key;
  bounds.makeEmpty();
  $("overlay").hidden = true;
  $("overlay").className = "";
  const is2d = view?.kind === "2d";
  if (view?.kind === "solid") {
    view.parts.forEach((part, index) => {
      const geometry = new THREE.BufferGeometry();
      geometry.setAttribute(
        "position",
        new THREE.BufferAttribute(decode(part.positions), 3),
      );
      geometry.computeVertexNormals();
      geometry.computeBoundingBox();
      const material = new THREE.MeshStandardMaterial({
        color: new THREE.Color(...part.color),
        metalness: 0.05,
        roughness: 0.65,
        flatShading: true,
        side: THREE.DoubleSide,
        polygonOffset: true,
        polygonOffsetFactor: 1,
        polygonOffsetUnits: 1,
      });
      const mesh = new THREE.Mesh(geometry, material);
      mesh.userData = { part: part.name, index };
      const edgeGeometry = part.edges
        ? new THREE.BufferGeometry().setAttribute(
            "position",
            new THREE.BufferAttribute(decode(part.edges), 3),
          )
        : new THREE.EdgesGeometry(geometry, 30);
      const edges = new THREE.LineSegments(
        edgeGeometry,
        new THREE.LineBasicMaterial({ color: 0x14161f }),
      );
      edges.userData.edges = true;
      mesh.add(edges);
      model.add(mesh);
      bounds.union(geometry.boundingBox);
    });
  } else if (is2d) {
    const material = new THREE.LineBasicMaterial({ color: 0x7aa2f7 });
    for (const path of view.paths) {
      const points = path.points.map(([x, y]) => new THREE.Vector3(x, y, 0));
      if (points.length === 1) continue;
      const geometry = new THREE.BufferGeometry().setFromPoints(points);
      const line = new (path.closed ? THREE.LineLoop : THREE.Line)(
        geometry,
        material,
      );
      line.userData = { part: path.layer || path.kind };
      model.add(line);
      geometry.computeBoundingBox();
      bounds.union(geometry.boundingBox);
    }
  } else if (view) {
    $("overlay").hidden = false;
    $("overlay").className = "value";
    $("overlay").textContent =
      view.kind === "value"
        ? `${state.run?.command ?? ""} = ${view.value}`
        : "Command emitted no geometry";
  }
  if (grid) scene.remove(grid);
  grid = null;
  if (!bounds.isEmpty()) {
    const size = bounds.getSize(new THREE.Vector3());
    const extent = Math.max(size.x, size.y, 1e-3);
    const step = 10 ** Math.floor(Math.log10(extent / 4));
    const cells = Math.ceil((extent * 1.6) / step / 2) * 2;
    grid = new THREE.GridHelper(cells * step, cells, 0x3b4261, 0x262a3a);
    grid.rotation.x = Math.PI / 2;
    const center = bounds.getCenter(new THREE.Vector3());
    grid.position.set(
      Math.round(center.x / step) * step,
      Math.round(center.y / step) * step,
      is2d ? -1e-3 : bounds.min.z,
    );
    grid.visible = $("grid").checked;
    scene.add(grid);
    raycaster.params.Line.threshold = extent / 200;
  }
  if (is2d) {
    $("ortho").checked = true;
    setCamera(true);
    $("view").value = "top";
  }
  controls.enableRotate = !is2d;
  materials();
  renderParts(view);
  fit();
  redrawMarks();
}

// ------------------------------------------------------------ pins/measure

function addMarker(point, text, kind, color, silent = false) {
  const size = bounds.isEmpty()
    ? 1
    : bounds.getSize(new THREE.Vector3()).length() / 160;
  const sphere = new THREE.Mesh(
    new THREE.SphereGeometry(Math.max(size, 1e-4), 12, 8),
    new THREE.MeshBasicMaterial({ color, depthTest: false }),
  );
  sphere.renderOrder = 10;
  sphere.position.copy(point);
  marks.add(sphere);
  if (!silent) {
    const label = element("span", { className: kind, textContent: text });
    label.dataset.x = point.x;
    label.dataset.y = point.y;
    label.dataset.z = point.z;
    $("labels").append(label);
  }
  return sphere;
}
function placeLabels() {
  const { clientWidth: w, clientHeight: h } = $("viewport");
  for (const label of $("labels").children) {
    const p = new THREE.Vector3(
      +label.dataset.x,
      +label.dataset.y,
      +label.dataset.z,
    ).project(camera);
    label.style.display = p.z > 1 ? "none" : "";
    label.style.left = `${((p.x + 1) / 2) * w}px`;
    label.style.top = `${((1 - p.y) / 2) * h}px`;
  }
}
function pick(event) {
  const rect = canvas.getBoundingClientRect();
  const pointer = new THREE.Vector2(
    ((event.clientX - rect.left) / rect.width) * 2 - 1,
    -((event.clientY - rect.top) / rect.height) * 2 + 1,
  );
  camera.updateMatrixWorld();
  raycaster.setFromCamera(pointer, camera);
  const hits = raycaster
    .intersectObjects(
      model.children.filter((o) => o.visible),
      false,
    )
    .filter(
      (hit) =>
        !$("clip-axis").value || clipPlane.distanceToPoint(hit.point) >= -1e-9,
    );
  return hits[0] || null;
}
let down = null;
canvas.addEventListener("pointerdown", (e) => (down = [e.clientX, e.clientY]));
canvas.addEventListener("pointerup", (event) => {
  const mode = $("mode").value;
  if (!down || Math.hypot(event.clientX - down[0], event.clientY - down[1]) > 4)
    return;
  const hit = pick(event);
  if (mode === "orbit" || !hit) {
    if (hit) $("readout").textContent = describeHit(hit);
    return;
  }
  const point = hit.point.clone();
  if (mode === "pin") {
    const pin = {
      id: state.pins.length + 1,
      point: point.toArray().map((v) => +v.toPrecision(6)),
      part: hit.object.userData.part,
      normal: hit.face
        ? hit.face.normal.toArray().map((v) => +v.toFixed(4))
        : null,
    };
    state.pins.push(pin);
    addMarker(point, `#${pin.id} ${pin.part ?? ""}`, "pin", 0xe0af68);
  } else {
    if (state.measure.length === 2) clearMarks();
    state.measure.push(point.toArray());
    addMarker(
      point,
      state.measure.length === 1 ? "A" : "B",
      "measure",
      0x7dcfff,
    );
    if (state.measure.length === 2) measureLine();
  }
});
function describeHit(hit) {
  const p = hit.point;
  return `${hit.object.userData.part ?? ""}  x ${fmt(p.x)}  y ${fmt(p.y)}  z ${fmt(p.z)}`;
}
function measureLine() {
  const [a, b] = state.measure.map((p) => new THREE.Vector3(...p));
  const line = new THREE.Line(
    new THREE.BufferGeometry().setFromPoints([a, b]),
    new THREE.LineBasicMaterial({ color: 0x7dcfff, depthTest: false }),
  );
  line.renderOrder = 10;
  marks.add(line);
  const d = b.clone().sub(a);
  const mid = a.clone().add(b).multiplyScalar(0.5);
  const label = element("span", {
    className: "measure",
    textContent: `${fmt(d.length())} mm`,
  });
  Object.assign(label.dataset, { x: mid.x, y: mid.y, z: mid.z });
  $("labels").append(label);
  $("readout").textContent =
    `distance ${fmt(d.length())} mm  dx ${fmt(d.x)}  dy ${fmt(d.y)}  dz ${fmt(d.z)}`;
}
function redrawMarks() {
  for (const child of [...marks.children]) {
    child.geometry?.dispose();
    child.material?.dispose();
    marks.remove(child);
  }
  $("labels").replaceChildren();
  for (const datum of state.view?.datums || [])
    addMarker(
      new THREE.Vector3(...datum.origin),
      `${datum.part}:${datum.id}`,
      "datum",
      0xbb9af7,
    );
  for (const path of state.view?.kind === "2d" ? state.view.paths : [])
    if (path.points.length === 1)
      addMarker(
        new THREE.Vector3(...path.points[0], 0),
        "",
        "point",
        0xe0af68,
        true,
      );
  for (const pin of state.pins)
    addMarker(
      new THREE.Vector3(...pin.point),
      `#${pin.id} ${pin.part ?? ""}`,
      "pin",
      0xe0af68,
    );
  state.measure.forEach((p, i) =>
    addMarker(new THREE.Vector3(...p), i ? "B" : "A", "measure", 0x7dcfff),
  );
  if (state.measure.length === 2) measureLine();
}
function clearMarks() {
  state.pins = [];
  state.measure = [];
  $("readout").textContent = "";
  redrawMarks();
}

// -------------------------------------------------------------- side panel

function renderParts(view) {
  const parts = view?.parts || [];
  $("parts").replaceChildren(
    ...parts.map((part, index) => {
      const box = element("input", {
        type: "checkbox",
        checked: true,
        title: "Show part",
      });
      box.onchange = () => {
        model.children[index].visible = box.checked;
        materials();
      };
      const [r, g, b] = part.color.map((c) => Math.round(c * 255));
      return element(
        "label",
        {
          className: "part",
          title: part.material ? `material ${part.material}` : "",
        },
        box,
        element("i", {
          className: "swatch",
          style: `background: rgb(${r},${g},${b})`,
        }),
        element("span", { textContent: part.name }),
        element("small", { textContent: `${part.stats.triangles} tri` }),
      );
    }),
  );
  if (!parts.length)
    $("parts").textContent =
      view?.kind === "2d" ? `${view.paths.length} paths` : "No solids";
}

function renderStats(run) {
  const rows = [];
  const add = (name, value, cls = "") =>
    rows.push(
      element(
        "tr",
        {},
        element("td", { textContent: name }),
        element("td", { textContent: value, className: cls }),
      ),
    );
  const s = run?.stats || {};
  if (run)
    add(
      "Status",
      run.status,
      run.status === "ok" ? "good" : run.status === "running" ? "" : "bad",
    );
  if (run?.result) add("Result", run.result);
  if (s.bbox) {
    const size = s.bbox[1].map((v, i) => v - s.bbox[0][i]);
    add("Size", size.map((v) => fmt(v)).join(" x ") + " mm");
    add("Min", s.bbox[0].map((v) => fmt(v)).join(", "));
    add("Max", s.bbox[1].map((v) => fmt(v)).join(", "));
  }
  if (s.volume !== undefined)
    add("Volume", `${fmt(s.volume)} mm³ (${fmt(s.volume / 1000)} cm³)`);
  if (s.area !== undefined) add("Area", `${fmt(s.area)} mm²`);
  if (s.centroid)
    add(
      "Centroid",
      s.centroid.map((v) => fmt(Math.abs(v) < 1e-6 ? 0 : v)).join(", "),
    );
  if (s.triangles !== undefined) add("Triangles", String(s.triangles));
  if (s.parts !== undefined) add("Parts", String(s.parts));
  if (s.paths !== undefined) add("Paths", String(s.paths));
  if (s.watertight !== undefined)
    add(
      "Watertight",
      s.watertight
        ? "yes"
        : `no (${s.boundary_edges} open, ${s.nonmanifold_edges} non-manifold edges)`,
      s.watertight ? "good" : "bad",
    );
  if (s.bodies !== undefined) add("Bodies", String(s.bodies));
  if (s.euler_number !== undefined)
    add("Genus", String(Math.round(s.bodies - s.euler_number / 2)));
  if (run?.build_seconds !== undefined)
    add("Build time", `${fmt(run.build_seconds, 3)} s`);
  if (run?.representation) add("Representation", run.representation);
  if (run?.package)
    add(
      "Package",
      run.package.valid ? "valid" : "invalid",
      run.package.valid ? "good" : "bad",
    );
  $("stats").replaceChildren(...rows);
  const messages = [];
  if (run?.message) messages.push(element("div", { textContent: run.message }));
  for (const failure of run?.require_failures || [])
    if (run.message?.indexOf(failure.expression || failure.message) < 0)
      messages.push(
        element("div", {
          textContent: `${failure.message} ${failure.expression || ""}`,
        }),
      );
  for (const note of run?.notes || [])
    messages.push(element("div", { className: "note", textContent: note }));
  for (const message of run?.package?.messages || [])
    messages.push(element("div", { className: "note", textContent: message }));
  $("messages").replaceChildren(...messages);
}

function renderDownloads(run) {
  const links = [];
  const link = (name, text) =>
    links.push(
      element("a", {
        href: `/api/runs/${run.id}/files/${name}?download=1`,
        textContent: text,
        download: "",
      }),
    );
  if (run && run.status !== "running") {
    for (const [format, name] of Object.entries(run.exports || {}))
      link(name, format.toUpperCase());
    if (run.preview) link("preview.png", "PNG");
    if (run.package) link("package.ycpkg.zip", "Package");
    if (run.assembly) link(run.assembly, "Assembly JSON");
    if (run.kind !== "import") link("source.dsl", "Source");
    link("run.log", "Log");
  }
  $("downloads").replaceChildren(...links);
}

function renderLibrary(info) {
  const groups = [
    ["Packages", info.packages],
    ["Geometry files", info.imports],
  ];
  const nodes = [];
  for (const [title, items] of groups) {
    if (!items.length) continue;
    nodes.push(element("h3", { textContent: title }));
    for (const item of items) {
      const open = element("button", {
        className: "small",
        title: "Open in viewer",
        type: "button",
      });
      open.innerHTML = '<i data-lucide="eye"></i>';
      open.onclick = () =>
        startRun("/api/import", { file: item }).catch((e) => error(e.message));
      nodes.push(
        element(
          "div",
          { className: "item", title: item },
          element("span", { textContent: item }),
          open,
        ),
      );
    }
  }
  if (!nodes.length)
    nodes.push(
      element("small", { textContent: "No .ycpkg, STL, STEP or DXF files" }),
    );
  $("library").replaceChildren(...nodes);
  window.lucide?.createIcons();
}

// --------------------------------------------------------------- commands

function currentCommand() {
  return (
    state.listing?.commands.find((c) => c.name === $("command").value) || null
  );
}
function paramKey() {
  return `params:${$("file").value}:${$("command").value}`;
}
function isDefault(param, value) {
  return JSON.stringify(param.default) === JSON.stringify(value);
}
function renderParams() {
  const command = currentCommand();
  const saved = load(paramKey(), {});
  const form = $("params");
  form.replaceChildren();
  if (!command) {
    form.append(
      element("small", { textContent: state.listing?.error || "No commands" }),
    );
    return;
  }
  const groups = new Map();
  for (const param of command.params) {
    const ui = param.ui || {};
    const value = param.name in saved ? saved[param.name] : param.default;
    const expression =
      value && typeof value === "object" && !Array.isArray(value)
        ? value.expression
        : null;
    const wrapper = element("label", { className: "param" });
    wrapper.dataset.name = param.name;
    wrapper.append(
      element(
        "span",
        {},
        element("b", {
          textContent: ui.label || param.name,
          style: "font-weight: normal",
        }),
        element("small", {
          textContent: param.type + (param.required ? " *" : ""),
        }),
      ),
    );
    let input;
    const numeric = param.type === "float" || param.type === "int";
    if (param.type === "bool") {
      input = element("input", { type: "checkbox", checked: !!value });
    } else if (numeric) {
      input = element("input", {
        type: "number",
        step: ui.step ?? (param.type === "int" ? 1 : "any"),
      });
      if (ui.min !== undefined) input.min = ui.min;
      if (ui.max !== undefined) input.max = ui.max;
      input.value = expression ? "" : (value ?? "");
      if (expression) input.placeholder = expression;
    } else {
      input = element("input", { type: "text" });
      input.value = expression
        ? ""
        : typeof value === "string"
          ? value
          : value === null || value === undefined
            ? ""
            : JSON.stringify(value);
      if (expression) input.placeholder = expression;
    }
    input.name = param.name;
    input.dataset.type = param.type;
    if (numeric && ui.min !== undefined && ui.max !== undefined) {
      const slider = element("input", {
        type: "range",
        min: ui.min,
        max: ui.max,
        step: ui.step ?? (param.type === "int" ? 1 : "any"),
      });
      slider.value = input.value;
      slider.oninput = () => {
        input.value = slider.value;
        paramsChanged(false);
      };
      slider.onchange = () => paramsChanged(true);
      input.addEventListener("input", () => (slider.value = input.value));
      wrapper.append(element("div", { className: "pair" }, slider, input));
    } else wrapper.append(input);
    input.addEventListener("change", () => paramsChanged(true));
    const group = ui.group || "";
    if (!groups.has(group)) groups.set(group, []);
    groups.get(group).push(wrapper);
  }
  for (const [group, nodes] of groups) {
    if (!group) form.append(...nodes);
    else
      form.append(
        element(
          "fieldset",
          {},
          element("legend", { textContent: group }),
          ...nodes,
        ),
      );
  }
  markChanged();
}
function readParams() {
  const command = currentCommand();
  const values = {};
  for (const input of $("params").querySelectorAll("input[name]")) {
    const param = command.params.find((p) => p.name === input.name);
    let value;
    if (input.type === "checkbox") value = input.checked;
    else if (input.value === "") continue;
    else if (input.type === "number") value = Number(input.value);
    else if (param.type === "string") value = input.value;
    else {
      try {
        value = JSON.parse(input.value);
      } catch {
        value = input.value;
      }
    }
    if (!isDefault(param, value)) values[input.name] = value;
  }
  return values;
}
function markChanged() {
  const values = readParams();
  for (const node of $("params").querySelectorAll(".param"))
    node.classList.toggle("changed", node.dataset.name in values);
}
let paramTimer;
function paramsChanged(commit) {
  markChanged();
  store(paramKey(), readParams());
  clearTimeout(paramTimer);
  if ($("live").checked)
    paramTimer = setTimeout(
      () => build(true, true).catch((e) => error(e.message)),
      commit ? 50 : 500,
    );
}

async function loadFile(file) {
  if (!file) {
    state.listing = null;
    options($("command"), [], "");
    renderParams();
    return;
  }
  state.listing = await api(`/api/commands?file=${encodeURIComponent(file)}`);
  const commands = state.listing.commands.map((c) => [
    c.name,
    `${c.name} → ${c.return_type}`,
  ]);
  options($("command"), commands, load(`command:${file}`, $("command").value));
  if (!$("command").value && commands.length)
    $("command").value = commands[0][0];
  renderParams();
  if (state.listing.error) error(`${file}: ${state.listing.error}`);
  if ($("tab").value === "source") await showSource();
}

function buildBody() {
  const exports = [...$("exports").querySelectorAll("input:checked")].map(
    (i) => i.value,
  );
  const body = {
    file: $("file").value,
    command: $("command").value,
    params: readParams(),
    representation: $("representation").value || null,
    exports,
    strict_step: $("strict-step").checked,
  };
  if ($("sdf-cell").value) body.sdf_cell = Number($("sdf-cell").value);
  return body;
}
async function startRun(path, body) {
  error("");
  state.follow = true;
  $("follow").checked = true;
  await api(path, body);
  await refresh();
}
async function build(replace = false, automatic = false) {
  if (!$("file").value || !$("command").value) return;
  const body = buildBody();
  const signature = JSON.stringify(body);
  // Automatic (live) builds skip requests identical to the last one.
  if (automatic && signature === state.lastBuild) return;
  state.lastBuild = signature;
  await startRun("/api/build", { ...body, replace });
}

// ------------------------------------------------------------- runs/state

async function refresh() {
  const info = await api("/api/state");
  const first = !state.info;
  state.info = info;
  if (first) $("live").checked = load("live", true);
  if (info.error) error(info.error);
  const previousFile = $("file").value || load("file", "");
  options(
    $("file"),
    info.sources.map((s) => [s, s]),
    previousFile,
  );
  if (!$("file").value && info.sources.length)
    $("file").value = info.sources[0];
  if ($("file").value !== state.file) {
    state.file = $("file").value;
    await loadFile(state.file);
  }
  $("build").disabled = !info.sources.length;
  $("stop").disabled = !info.active;
  const running = info.runs.find((r) => r.id === info.active);
  if (socket?.readyState === WebSocket.OPEN)
    $("status").textContent = running
      ? `Building ${running.label}`
      : info.capabilities?.brep
        ? "Ready · BREP"
        : "Ready";
  options(
    $("runs"),
    info.runs.map((r) => [
      r.id,
      `${r.label} · ${r.status} · ${r.started.slice(11, 19)}`,
    ]),
    $("runs").value,
  );
  if (state.follow && info.runs.length) $("runs").value = info.runs[0].id;
  renderLibrary(info);
  const run = info.runs.find((r) => r.id === $("runs").value);
  if (run && (run.id !== state.run?.id || run.status !== state.run?.status))
    await loadRun(run);
  if (!run) renderStats(null);
}
let ticker;
function adoptRun(run) {
  // Reflect builds started elsewhere (e.g. the agent's CLI) in the form.
  if (
    !state.follow ||
    run.kind === "import" ||
    !run.command ||
    $("params").contains(document.activeElement)
  )
    return;
  if (
    run.file !== $("file").value ||
    !state.listing?.commands.some((c) => c.name === run.command)
  )
    return;
  const changed =
    run.command !== $("command").value ||
    JSON.stringify(run.params) !== JSON.stringify(readParams());
  if (!changed) return;
  $("command").value = run.command;
  store(`command:${run.file}`, run.command);
  store(paramKey(), run.params || {});
  renderParams();
}
async function loadRun(run) {
  state.run = run;
  adoptRun(run);
  const epoch = ++state.epoch;
  renderStats(run);
  renderDownloads(run);
  $("result").textContent = run.status;
  $("result").className = run.status === "ok" ? "ok" : "failed";
  clearInterval(ticker);
  if (run.status === "running") {
    const started = Date.parse(run.started);
    const tick = () => {
      $("overlay").hidden = false;
      $("overlay").className = "";
      $("overlay").textContent =
        `Building ${run.label}\n${Math.round((Date.now() - started) / 1000)} s`;
    };
    tick();
    ticker = setInterval(tick, 1000);
  }
  const log = await fetch(`/api/runs/${run.id}/log`).then((r) => r.text());
  if (epoch !== state.epoch) return;
  $("log").textContent = log;
  if (run.status === "running") return;
  if (!run.preview && !["ok", "failed"].includes(run.status)) {
    $("overlay").hidden = false;
    $("overlay").textContent = run.message || run.status;
    return;
  }
  try {
    const view = await fetch(`/api/runs/${run.id}/files/view.json`).then((r) =>
      r.ok ? r.json() : null,
    );
    if (epoch !== state.epoch) return;
    state.view = view;
    showView(view);
    if (!view) {
      $("overlay").hidden = false;
      $("overlay").textContent = run.message || "No geometry";
    }
  } catch (e) {
    error(e.message);
  }
}

// ------------------------------------------------------- source/reference

async function showSource() {
  const file = $("file").value;
  if (!file) return;
  const [text, report] = await Promise.all([
    fetch(`/api/source?file=${encodeURIComponent(file)}`).then((r) => r.text()),
    api("/api/check", { file }).catch((e) => ({
      diagnostics: [{ severity: "error", message: e.message, line: null }],
    })),
  ]);
  const commandLines = new Set(
    (state.listing?.commands || []).map((c) => c.line),
  );
  const byLine = new Map();
  for (const d of report.diagnostics) {
    if (!byLine.has(d.line)) byLine.set(d.line, []);
    byLine.get(d.line).push(d);
  }
  const lines = [];
  text.split("\n").forEach((line, index) => {
    const number = index + 1;
    const diags = byLine.get(number) || [];
    const node = element("div", { textContent: line || " " });
    if (commandLines.has(number)) node.classList.add("command");
    if (diags.length)
      node.classList.add(
        diags.some((d) => d.severity === "error") ? "error" : "warning",
      );
    lines.push(node);
    for (const d of diags)
      lines.push(
        element("div", {
          className: "diag",
          textContent: `      ${d.severity} ${d.code ?? ""}: ${d.message}`,
        }),
      );
  });
  for (const d of byLine.get(null) || [])
    lines.unshift(
      element("div", { className: "diag", textContent: d.message }),
    );
  $("source").replaceChildren(...lines);
}
async function showReference() {
  if (!state.reference) state.reference = await api("/api/reference");
  const query = $("ref-search").value.trim().toLowerCase();
  const ref = state.reference;
  const matches = (...fields) =>
    !query ||
    fields.some((f) =>
      String(f ?? "")
        .toLowerCase()
        .includes(query),
    );
  const nodes = [];
  const functions = Object.values(ref.functions).filter((f) =>
    matches(f.name, f.signature, f.description),
  );
  if (functions.length)
    nodes.push(
      element("h3", { textContent: `Functions (${functions.length})` }),
    );
  for (const f of functions)
    nodes.push(
      element(
        "div",
        {},
        element("code", { textContent: f.signature }),
        f.description ? element("p", { textContent: f.description }) : null,
        f.example ? element("pre", { textContent: f.example }) : null,
      ),
    );
  for (const [type, methods] of Object.entries(ref.methods)) {
    const list = Object.values(methods).filter((m) =>
      matches(type, m.name, m.signature, m.description),
    );
    if (!list.length) continue;
    nodes.push(element("h3", { textContent: `${type} methods` }));
    for (const m of list)
      nodes.push(
        element(
          "div",
          {},
          element("code", { textContent: `${type}.${m.signature}` }),
          m.description ? element("p", { textContent: m.description }) : null,
        ),
      );
  }
  const types = Object.entries(ref.types).filter(([name, t]) =>
    matches(name, t.description),
  );
  if (types.length) nodes.push(element("h3", { textContent: "Types" }));
  for (const [name, t] of types)
    nodes.push(
      element(
        "div",
        {},
        element("code", { textContent: name }),
        element("p", {
          textContent: `${t.description ?? ""} ${t.examples ? "e.g. " + t.examples.join(", ") : ""}`,
        }),
      ),
    );
  $("ref-list").replaceChildren(...nodes);
}
async function switchTab() {
  const tab = $("tab").value;
  $("viewport").hidden = tab !== "model";
  $("model-tools").hidden = tab !== "model";
  $("source").hidden = tab !== "source";
  $("reference").hidden = tab !== "reference";
  if (tab === "source") await showSource();
  if (tab === "reference") await showReference();
  if (tab === "model") resize();
}

// -------------------------------------------------------------- annotation

$("annotate").onclick = () => {
  if (!state.run) return;
  renderer.render(scene, camera);
  const capture = document.createElement("canvas");
  const scale = Math.min(1, 1200 / canvas.width);
  capture.width = Math.round(canvas.width * scale);
  capture.height = Math.round(canvas.height * scale);
  const ctx = capture.getContext("2d");
  ctx.drawImage(canvas, 0, 0, capture.width, capture.height);
  // Burn pin and measurement labels into the capture.
  ctx.font = `${Math.round(13 * scale * devicePixelRatio)}px system-ui`;
  const rect = $("viewport").getBoundingClientRect();
  for (const label of $("labels").children) {
    if (label.style.display === "none") continue;
    const x = (parseFloat(label.style.left) / rect.width) * capture.width;
    const y = (parseFloat(label.style.top) / rect.height) * capture.height - 10;
    ctx.fillStyle = getComputedStyle(label).backgroundColor;
    const width = ctx.measureText(label.textContent).width + 8;
    ctx.fillRect(x - width / 2, y - 14, width, 18);
    ctx.fillStyle = "#1a1b26";
    ctx.fillText(label.textContent, x - width / 2 + 4, y);
  }
  const run = state.run;
  const visible = model.children
    .filter((m) => m.visible)
    .map((m) => m.userData.part);
  state.note = {
    title: `${run.label}${state.pins.length ? ` · ${state.pins.length} pin(s)` : ""}`,
    context: {
      plugin: "yapcad",
      project: state.info.project,
      run: run.id,
      runPath: run.path,
      file: run.file,
      command: run.command,
      params: run.params,
      representation: run.representation,
      revision: run.revision,
      status: run.status,
      stats: run.stats,
      pins: state.pins,
      measure:
        state.measure.length === 2
          ? { a: state.measure[0], b: state.measure[1] }
          : null,
      camera: {
        projection: camera === orthographic ? "orthographic" : "perspective",
        position: camera.position.toArray(),
        target: controls.target.toArray(),
        up: camera.up.toArray(),
      },
      section: $("clip-axis").value
        ? {
            axis: $("clip-axis").value,
            normal: clipPlane.normal.toArray(),
            constant: clipPlane.constant,
          }
        : null,
      visibleParts: visible.length === model.children.length ? "all" : visible,
      units: "mm",
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
          plugin: "yapcad",
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
$("record").onclick = () => {
  if (recognition) return recognition.stop();
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
    $("record").classList.remove("on");
  };
  recognition.start();
  $("record").classList.add("on");
};
$("note").addEventListener("close", () => recognition?.stop());

// ------------------------------------------------------------------ wiring

const report = (promise) => promise.catch((e) => error(e.message));
$("sidebar").onclick = () => {
  document.body.classList.toggle("collapsed");
  resize();
};
$("file").onchange = () => {
  store("file", $("file").value);
  state.file = $("file").value;
  report(loadFile(state.file));
};
$("command").onchange = () => {
  store(`command:${$("file").value}`, $("command").value);
  renderParams();
};
$("reset").onclick = () => {
  store(paramKey(), {});
  renderParams();
  paramsChanged(true);
};
$("build").onclick = () => report(build());
$("package").onclick = () =>
  report(
    startRun("/api/package", {
      ...buildBody(),
      package: { name: $("command").value.toLowerCase() },
    }),
  );
$("stop").onclick = () => report(api("/api/cancel", {}).then(refresh));
$("runs").onchange = () => {
  state.follow = false;
  $("follow").checked = false;
  const run = state.info.runs.find((r) => r.id === $("runs").value);
  if (run) report(loadRun(run));
};
$("follow").onchange = () => {
  state.follow = $("follow").checked;
  report(refresh());
};
$("live").onchange = () => store("live", $("live").checked);
$("tab").onchange = () => report(switchTab());
$("ref-search").oninput = () => report(showReference());
$("fit").onclick = () => fit();
$("view").onchange = () => fit();
$("ortho").onchange = () => setCamera($("ortho").checked);
for (const id of ["edges", "wire"]) $(id).onchange = materials;
$("grid").onchange = () => grid && (grid.visible = $("grid").checked);
function updateClip() {
  const axis = $("clip-axis").value;
  $("clip").hidden = $("clip-flip").hidden = !axis;
  if (axis && !bounds.isEmpty()) {
    const index = { x: 0, y: 1, z: 2 }[axis];
    const normal = new THREE.Vector3().setComponent(
      index,
      $("clip-flip").dataset.flip ? 1 : -1,
    );
    const t = Number($("clip").value) / 1000;
    const position =
      bounds.min.getComponent(index) +
      t * (bounds.max.getComponent(index) - bounds.min.getComponent(index));
    clipPlane.normal.copy(normal);
    clipPlane.constant = -normal.getComponent(index) * position;
    $("readout").textContent = `section ${axis} = ${fmt(position)} mm`;
  }
  materials();
}
$("clip-axis").onchange = updateClip;
$("clip").oninput = updateClip;
$("clip-flip").onclick = () => {
  $("clip-flip").dataset.flip = $("clip-flip").dataset.flip ? "" : "1";
  updateClip();
};
$("mode").onchange = () => {
  $("viewport").className = $("mode").value;
  controls.enabled = true;
};
$("clear-marks").onclick = clearMarks;
document.addEventListener("keydown", (e) => {
  if ((e.ctrlKey || e.metaKey) && e.key === "Enter") report(build());
  if (e.target.matches("input, textarea, select")) return;
  if (e.key === "f") fit();
});

function connect() {
  socket = new WebSocket(
    `${location.protocol === "https:" ? "wss:" : "ws:"}//${location.host}/ws`,
  );
  socket.onopen = () => report(refresh());
  socket.onmessage = (event) => {
    const message = JSON.parse(event.data);
    if (message.type === "files") {
      const current = $("file").value;
      if (message.files?.includes(current)) {
        state.lastBuild = null;
        report(
          loadFile(current).then(() => {
            if ($("live").checked) return build(true);
          }),
        );
      }
    }
    report(refresh());
  };
  socket.onclose = () => {
    $("status").textContent = "Disconnected";
    setTimeout(connect, 1500);
  };
  socket.onerror = () => socket.close();
}
resize();
animate();
connect();
