import { installCanvasPresentation } from "./canvas-presentation.js";
import { installSchematicSizing } from "./schematic-sizing.js";
import { installNativeTouch } from "./native-touch.js";
import { installMobileProperties } from "./properties-mobile.js";

// A frame owns each renderer's workers and GPU lifetime.
const host = document.getElementById("viewer");
const error = document.getElementById("error");
let viewer;
let chain = Promise.resolve();
let revision;
let probeId;
let netHighlightId;
let model;
let modelRenderer;
let latestSelection;
let nativeActive = false;
let viewOptions = { polygonPours: true };
let nativeContext = "unknown";
let modelContext = "model";
let nativeViewTimer;
const send = (message) =>
  parent.postMessage(message, location.origin === "null" ? "*" : location.origin);
const applyViewOptions = () => {
  viewer?.setPolygonPoursVisible?.(viewOptions.polygonPours);
};
const applyViewOptionsSoon = () => {
  requestAnimationFrame(() => applyViewOptions());
  setTimeout(applyViewOptions, 80);
  setTimeout(applyViewOptions, 250);
};
const sendViewerState = (context, view, ui) => {
  if (!view && !ui) return;
  send({ type: "kicad-pcb-view-state", context, view, ui });
};
const reportNativeViewerState = () => {
  sendViewerState(nativeContext, viewer?.captureView?.(), viewer?.captureUiState?.());
};
const scheduleNativeViewerState = (delay = 140) => {
  clearTimeout(nativeViewTimer);
  nativeViewTimer = setTimeout(reportNativeViewerState, delay);
};
const reportModelViewerState = () => {
  sendViewerState(modelContext, modelRenderer?.captureView?.(), modelRenderer?.captureUiState?.());
};
const installNativeStateListeners = () => {
  if (host.__kicadPcbNativeStateListeners) return;
  host.__kicadPcbNativeStateListeners = true;
  for (const type of ["pointerup", "pointercancel", "wheel", "keyup", "touchend"])
    host.addEventListener(type, () => scheduleNativeViewerState(), {
      capture: true,
      passive: true,
    });
};
const editableTarget = (target) =>
  target instanceof Element &&
  (target.isContentEditable || /^(?:INPUT|TEXTAREA|SELECT)$/.test(target.tagName));
window.addEventListener("keydown", (event) => {
  if (
    !nativeActive ||
    event.repeat ||
    event.defaultPrevented ||
    event.ctrlKey ||
    event.metaKey ||
    event.altKey ||
    event.composedPath().some(editableTarget)
  )
    return;
  if (
    event.key === "x" ||
    event.key === "X" ||
    event.key === "h" ||
    event.key === "H" ||
    event.key === "Escape"
  ) {
    if (event.key === "Escape" || event.key === "x" || event.key === "X")
      viewer?.clearNetHighlight?.();
    else if (event.key === "h" || event.key === "H")
      viewer?.requestNetHighlight?.(latestSelection);
    send({ type: "kicad-pcb-native-key", key: event.key });
  }
});
window.addEventListener("message", (event) => {
  if (event.source !== parent || event.origin !== location.origin) return;
  if (event.data?.type === "kicad-pcb-view-options") {
    viewOptions = { ...viewOptions, polygonPours: event.data.polygonPours !== false };
    applyViewOptions();
    applyViewOptionsSoon();
    return;
  }
  if (event.data?.type !== "kicad-pcb-snapshot") return;
  const snapshot = event.data;
  viewOptions = { ...viewOptions, polygonPours: snapshot.polygonPours !== false };
  if (snapshot.kind === "model" || snapshot.kind === "step") {
    modelContext = snapshot.context || snapshot.kind || "model";
    nativeActive = false;
    void (async () => {
      if (!model) {
        model = import("./board-model.js").then(({ createBoardModel }) =>
          createBoardModel(host, error, {
            kind: snapshot.kind,
            subject: snapshot.subject,
            onViewChange: reportModelViewerState,
            onUiStateChange: reportModelViewerState,
          }),
        );
      }
      const renderer = await model;
      modelRenderer = renderer;
      renderer.setActive(snapshot.active !== false);
      if (snapshot.active !== false) {
        renderer.restoreUiState?.(snapshot.uiState);
        await renderer.update(snapshot.url);
        renderer.restoreUiState?.(snapshot.uiState);
        renderer.restoreView?.(snapshot.viewState);
        reportModelViewerState();
      }
    })().catch((cause) => {
      error.textContent = cause.message || String(cause);
    });
    return;
  }
  nativeContext = snapshot.context || "unknown";
  nativeActive = snapshot.active !== false;
  chain = chain
    .then(async () => {
      error.textContent = "";
      {
        if (!viewer) {
          host.style.visibility = "hidden";
          await import("./ecad-viewer.js");
          const { RetainedNativeViewer } = await import("./retained-native-viewer.js");
          viewer = new RetainedNativeViewer(host);
          viewer.addEventListener("selection", (event) => {
            latestSelection = event.detail;
            send({
              type: "kicad-pcb-selection",
              selection: event.detail,
              userInitiated: event.detail?.userInitiated === true,
            });
          });
          viewer.addEventListener("crossprobe", (event) =>
            send({
              type: "kicad-pcb-crossprobe",
              selection: event.detail,
              userInitiated: event.detail?.userInitiated === true,
            }),
          );
          viewer.addEventListener("layers", (event) =>
            send({ type: "kicad-pcb-layers", layers: event.detail }),
          );
          viewer.addEventListener("viewstatechange", () => scheduleNativeViewerState(20));
          installNativeStateListeners();
        }
        if (revision !== snapshot.revision) {
          netHighlightId = undefined;
          error.classList.toggle("refresh-status", Boolean(viewer.current));
          error.textContent = viewer.current ? "Updating preview…" : "Loading preview…";
          await viewer.replaceSources({
            revisionKey: snapshot.revision,
            sources: snapshot.sources,
            layerVisibility: snapshot.layerVisibility,
          });
          revision = snapshot.revision;
          error.textContent = "";
        }
        await viewer.ready;
        viewer.enhanceGeometrySelection();
        if (snapshot.layerVisibility) {
          for (const [name, visible] of Object.entries(snapshot.layerVisibility))
            viewer.setLayerVisibility(name, Boolean(visible));
        }
        viewer.activateContext?.(snapshot.context);
        applyViewOptions();
        viewer.setActive(snapshot.active !== false);
        if (snapshot.active !== false) {
          viewer.resize();
          const native = viewer.current;
          if (native) {
            viewer.activateContext?.(snapshot.context);
            await installCanvasPresentation(native);
            viewer.restoreUiState?.(snapshot.uiState);
            viewer.restoreView?.(snapshot.viewState);
            viewer.reseedLayerCache();
            viewer.enhanceGeometrySelection();
            applyViewOptions();
            applyViewOptionsSoon();
            viewer.publishLayers();
            host.style.visibility = "visible";
            installSchematicSizing(native);
            installNativeTouch(native);
            installMobileProperties(native);
            scheduleNativeViewerState(60);
          }
          if (snapshot.probe && probeId !== snapshot.probe.id) {
            probeId = snapshot.probe.id;
            const found = viewer.requestCrossProbe(snapshot.probe);
            send({ type: "kicad-pcb-probe-result", found, value: snapshot.probe.value });
          }
          if (snapshot.netHighlight && netHighlightId !== snapshot.netHighlight.id) {
            netHighlightId = snapshot.netHighlight.id;
            const applies =
              !snapshot.netHighlight.targetContext ||
              snapshot.netHighlight.targetContext === snapshot.context;
            viewer.requestNetHighlight?.(
              applies || snapshot.netHighlight.clear
                ? snapshot.netHighlight
                : { ...snapshot.netHighlight, clear: true },
            );
          }
        }
      }
    })
    .catch((cause) => {
      error.textContent = cause.message || String(cause);
    });
});
window.KicadViewerCapture = async () => {
  if (model) {
    const renderer = modelRenderer ?? (await model);
    const image = renderer?.capture?.();
    if (image) return image;
  }
  await chain.catch(() => {});
  return viewer?.captureImage?.() ?? null;
};
parent.postMessage(
  { type: "kicad-pcb-runtime-ready" },
  location.origin === "null" ? "*" : location.origin,
);
