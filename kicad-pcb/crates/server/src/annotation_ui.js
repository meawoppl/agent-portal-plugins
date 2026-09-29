// KiCad PCB annotation stack. Served from /kicad-pcb/annotations.js.
(() => {
  const EDIT_STACK_ENDPOINT = "/__portal/edit-stack";
  const state = {
    armed: false,
    stack: [],
    selection: undefined,
    recognition: undefined,
    recognizing: false,
    sending: false,
  };

  const style = document.createElement("style");
  style.textContent = `
.annotation-button,.annotation-action{border:1px solid #3b4261;border-radius:6px;background:#24283b;color:#c0caf5;padding:7px 10px;cursor:pointer}
.annotation-button.active{background:#bb9af7;color:#10131d;border-color:#bb9af7}
.annotation-button:disabled,.annotation-action:disabled{opacity:.55;cursor:not-allowed}
.annotation-overlay{position:fixed;inset:0;z-index:80;display:none;cursor:crosshair;background:rgba(17,19,29,.18)}
.annotation-overlay.active{display:block}
.annotation-rect{position:fixed;border:2px solid #bb9af7;background:rgba(187,154,247,.16);box-shadow:0 0 0 9999px rgba(17,19,29,.22);pointer-events:none}
.annotation-drawer{position:fixed;right:14px;bottom:14px;z-index:90;width:min(420px,calc(100vw - 28px));max-height:calc(100vh - 28px);display:flex;flex-direction:column;gap:10px;border:1px solid #3b4261;border-radius:8px;background:#1a1b26;color:#c0caf5;box-shadow:0 18px 48px rgba(0,0,0,.45);padding:12px}
.annotation-drawer.hidden{display:none}
.annotation-drawer h2{font-size:14px;margin:0;color:#e6e9f5}
.annotation-drawer textarea,.annotation-drawer input{width:100%;box-sizing:border-box;border:1px solid #3b4261;border-radius:6px;background:#11131d;color:#c0caf5;padding:8px;font:inherit}
.annotation-drawer textarea{min-height:94px;resize:vertical}
.annotation-row{display:flex;gap:8px;align-items:center;justify-content:space-between}
.annotation-actions{display:flex;gap:8px;flex-wrap:wrap;justify-content:flex-end}
.annotation-preview{border:1px solid #3b4261;border-radius:6px;background:#11131d;min-height:38px;max-height:140px;display:flex;align-items:center;justify-content:center;overflow:hidden;cursor:pointer}
.annotation-preview img{max-width:100%;max-height:140px;display:block}
.annotation-stack{display:grid;gap:8px;overflow:auto;max-height:210px}
.annotation-card{border:1px solid #3b4261;border-radius:6px;background:#181b29;padding:8px;display:grid;gap:4px}
.annotation-card-title{font-size:12px;color:#e6e9f5;display:flex;justify-content:space-between;gap:8px}
.annotation-card-body{font-size:12px;color:#9aa5ce;white-space:pre-wrap;max-height:56px;overflow:hidden}
.annotation-remove{border:0;background:transparent;color:#f7768e;cursor:pointer;font:inherit}
`;
  document.head.append(style);

  const overlay = document.createElement("div");
  overlay.className = "annotation-overlay";
  const rectEl = document.createElement("div");
  rectEl.className = "annotation-rect";
  overlay.append(rectEl);
  document.body.append(overlay);

  const drawer = document.createElement("aside");
  drawer.className = "annotation-drawer hidden";
  drawer.innerHTML = `
<div class="annotation-row">
  <h2>Annotation Stack</h2>
  <button class="annotation-action" data-action="close">Close</button>
</div>
<div class="annotation-preview"><span class="muted">Click to select an area, or submit a note without image.</span></div>
<input class="annotation-title" placeholder="Short title">
<textarea class="annotation-note" placeholder="Type a note, or use Record if this browser supports speech recognition."></textarea>
<div class="annotation-row">
  <span class="muted annotation-status">No queued annotations.</span>
  <div class="annotation-actions">
    <button class="annotation-action" data-action="select">Select Area</button>
    <button class="annotation-action" data-action="record">Record</button>
    <button class="annotation-action" data-action="add">Submit</button>
  </div>
</div>
<div class="annotation-stack"></div>
<div class="annotation-actions">
  <button class="annotation-action" data-action="clear">Clear</button>
</div>`;
  document.body.append(drawer);

  const titleInput = drawer.querySelector(".annotation-title");
  const noteInput = drawer.querySelector(".annotation-note");
  const preview = drawer.querySelector(".annotation-preview");
  const stackEl = drawer.querySelector(".annotation-stack");
  const statusEl = drawer.querySelector(".annotation-status");
  const recordButton = drawer.querySelector('[data-action="record"]');
  const addButton = drawer.querySelector('[data-action="add"]');

  const button = document.createElement("button");
  button.type = "button";
  button.className = "annotation-button";
  button.textContent = "Annotate";
  button.title = "Select a view region and queue an agent prompt";
  document.querySelector(".header-tools")?.prepend(button);

  const speechCtor = window.SpeechRecognition || window.webkitSpeechRecognition;
  if (!speechCtor) {
    recordButton.disabled = true;
    recordButton.title = "Speech recognition is not available in this browser.";
  }

  const activeSection = () => document.querySelector("main section.active");
  const activeTab = () => activeSection()?.id || "unknown";
  const sourceRevision = () => window.KicadWorkbenchState?.sourceRevision?.() || undefined;
  const emptyPreviewText = "Click to select an area, or submit a note without image.";

  const setStatus = (text) => {
    statusEl.textContent = text;
  };

  const setArmed = (armed) => {
    state.armed = armed;
    button.classList.toggle("active", armed);
    overlay.classList.toggle("active", armed);
    if (armed) {
      drawer.classList.add("hidden");
      setStatus("Drag a region in the current view.");
    }
  };

  const normalizeRect = (rect, base) => ({
    x: round((rect.left - base.left) / Math.max(1, base.width)),
    y: round((rect.top - base.top) / Math.max(1, base.height)),
    w: round(rect.width / Math.max(1, base.width)),
    h: round(rect.height / Math.max(1, base.height)),
  });

  const round = (value) => Math.round(value * 10000) / 10000;

  const rectFromPoints = (a, b) => {
    const left = Math.min(a.x, b.x);
    const top = Math.min(a.y, b.y);
    const width = Math.abs(a.x - b.x);
    const height = Math.abs(a.y - b.y);
    if (width < 8 && height < 8) {
      return {
        left: Math.max(0, a.x - 24),
        top: Math.max(0, a.y - 24),
        width: 48,
        height: 48,
      };
    }
    return { left, top, width, height };
  };

  const setRectEl = (rect) => {
    rectEl.style.left = `${rect.left}px`;
    rectEl.style.top = `${rect.top}px`;
    rectEl.style.width = `${rect.width}px`;
    rectEl.style.height = `${rect.height}px`;
  };

  const viewportRect = (rect) => ({
    left: rect.left,
    top: rect.top,
    right: rect.left + rect.width,
    bottom: rect.top + rect.height,
    width: rect.width,
    height: rect.height,
  });

  const intersect = (a, b) => {
    const left = Math.max(a.left, b.left);
    const top = Math.max(a.top, b.top);
    const right = Math.min(a.right, b.right);
    const bottom = Math.min(a.bottom, b.bottom);
    if (right <= left || bottom <= top) return undefined;
    return { left, top, right, bottom, width: right - left, height: bottom - top };
  };

  const visibleRect = (rect) => rect.width > 1 && rect.height > 1;

  const visibleElement = (element) => {
    const view = element.ownerDocument?.defaultView || window;
    const style = view.getComputedStyle?.(element);
    if (!style) return true;
    return (
      style.display !== "none" &&
      style.visibility !== "hidden" &&
      Number(style.opacity || "1") > 0.01
    );
  };

  const adjustedRect = (rect, offset) => ({
    left: offset.left + rect.left,
    top: offset.top + rect.top,
    right: offset.left + rect.right,
    bottom: offset.top + rect.bottom,
    width: rect.width,
    height: rect.height,
  });

  const canvasCandidatesFromRoot = (root, offset = { left: 0, top: 0 }) => {
    const candidates = [];
    root.querySelectorAll?.("canvas").forEach((canvas) => {
      const rect = adjustedRect(canvas.getBoundingClientRect(), offset);
      if (visibleRect(rect) && visibleElement(canvas)) candidates.push({ canvas, rect });
    });
    root.querySelectorAll?.("*").forEach((node) => {
      if (node.shadowRoot) candidates.push(...canvasCandidatesFromRoot(node.shadowRoot, offset));
      if (node.tagName !== "IFRAME") return;
      try {
        const frameRect = adjustedRect(node.getBoundingClientRect(), offset);
        if (visibleRect(frameRect) && node.contentDocument) {
          candidates.push(
            ...canvasCandidatesFromRoot(node.contentDocument, {
              left: frameRect.left,
              top: frameRect.top,
            }),
          );
        }
      } catch (_) {
        // Cross-origin or not ready; prompt context still carries the region.
      }
    });
    return candidates;
  };

  const canvasCandidates = () => {
    const section = activeSection();
    const scoped = section ? canvasCandidatesFromRoot(section) : [];
    return scoped.length ? scoped : canvasCandidatesFromRoot(document);
  };

  const loadImage = (dataUrl) =>
    new Promise((resolve, reject) => {
      const image = new Image();
      image.onload = () => resolve(image);
      image.onerror = () => reject(new Error("Captured image could not be decoded."));
      image.src = dataUrl;
    });

  const captureLooksBlank = (context, width, height) => {
    try {
      const data = context.getImageData(0, 0, width, height).data;
      let minChannel = 255;
      let maxChannel = 0;
      let opaqueSamples = 0;
      const pixelCount = Math.max(1, width * height);
      const pixelStride = Math.max(1, Math.ceil(pixelCount / 20000));
      for (let pixel = 0; pixel < pixelCount; pixel += pixelStride) {
        const i = pixel * 4;
        if (data[i + 3] < 8) continue;
        opaqueSamples += 1;
        const localMin = Math.min(data[i], data[i + 1], data[i + 2]);
        const localMax = Math.max(data[i], data[i + 1], data[i + 2]);
        minChannel = Math.min(minChannel, localMin);
        maxChannel = Math.max(maxChannel, localMax);
      }
      return opaqueSamples === 0 || (maxChannel < 32 && maxChannel - minChannel < 18);
    } catch (_) {
      return false;
    }
  };

  const cropSource = (source, sourceWidth, sourceHeight, surfaceRect, hit) => {
    const scaleX = sourceWidth / Math.max(1, surfaceRect.width);
    const scaleY = sourceHeight / Math.max(1, surfaceRect.height);
    const sx = Math.max(0, (hit.left - surfaceRect.left) * scaleX);
    const sy = Math.max(0, (hit.top - surfaceRect.top) * scaleY);
    const sw = Math.max(1, Math.min(sourceWidth - sx, hit.width * scaleX));
    const sh = Math.max(1, Math.min(sourceHeight - sy, hit.height * scaleY));
    const maxSide = 900;
    const ratio = Math.min(1, maxSide / Math.max(sw, sh));
    const out = document.createElement("canvas");
    out.width = Math.max(1, Math.round(sw * ratio));
    out.height = Math.max(1, Math.round(sh * ratio));
    const context = out.getContext("2d", { willReadFrequently: true });
    context.drawImage(source, sx, sy, sw, sh, 0, 0, out.width, out.height);
    return {
      dataUrl: out.toDataURL("image/png"),
      blank: captureLooksBlank(context, out.width, out.height),
    };
  };

  const cropDataUrl = async (capture, surfaceRect, hit) => {
    const image = await loadImage(capture.image);
    return cropSource(
      image,
      capture.width || image.naturalWidth || image.width,
      capture.height || image.naturalHeight || image.height,
      surfaceRect,
      hit,
    );
  };

  const withTimeout = (promise, ms) =>
    new Promise((resolve) => {
      const timer = setTimeout(() => resolve(undefined), ms);
      Promise.resolve(promise).then(
        (value) => {
          clearTimeout(timer);
          resolve(value);
        },
        () => {
          clearTimeout(timer);
          resolve(undefined);
        },
      );
    });

  const viewerFrameCandidates = (selection) => {
    const section = activeSection();
    const root = section || document;
    return [...root.querySelectorAll("iframe.native-viewer,iframe.model-viewer")]
      .map((frame) => {
        const rect = viewportRect(frame.getBoundingClientRect());
        const hit = intersect(selection, rect);
        return hit ? { frame, rect, hit } : undefined;
      })
      .filter(Boolean)
      .sort((a, b) => b.hit.width * b.hit.height - a.hit.width * a.hit.height);
  };

  const captureFromViewerFrame = async (selection) => {
    let blank = false;
    for (const candidate of viewerFrameCandidates(selection)) {
      const api = candidate.frame.contentWindow?.KicadViewerCapture;
      if (typeof api !== "function") continue;
      const capture = await withTimeout(api(), 1800);
      if (!capture?.image) continue;
      try {
        const cropped = await cropDataUrl(capture, candidate.rect, candidate.hit);
        if (!cropped.blank) return { dataUrl: cropped.dataUrl };
        blank = true;
      } catch (_) {
        // Fall through to direct canvas capture below.
      }
    }
    return blank
      ? { note: "Capture surface was blank; try again after the view finishes rendering." }
      : undefined;
  };

  const captureFromCanvasCandidates = (selection) => {
    const candidates = [];
    for (const candidate of canvasCandidates()) {
      const canvasRect = viewportRect(candidate.rect);
      const hit = intersect(selection, canvasRect);
      if (!hit) continue;
      candidates.push({ ...candidate, hit, canvasRect });
    }
    candidates.sort((a, b) => b.hit.width * b.hit.height - a.hit.width * a.hit.height);
    if (!candidates.length) return { note: "No capture canvas was available for this view." };
    let blank = false;
    try {
      for (const candidate of candidates) {
        const cropped = cropSource(
          candidate.canvas,
          candidate.canvas.width,
          candidate.canvas.height,
          candidate.canvasRect,
          candidate.hit,
        );
        if (!cropped.blank) return { dataUrl: cropped.dataUrl };
        blank = true;
      }
      return blank
        ? { note: "Capture canvas rendered blank; try again after the view finishes rendering." }
        : { note: "No readable capture canvas was available for this view." };
    } catch (err) {
      return { note: `Capture unavailable: ${err?.message || err}` };
    }
  };

  const captureSelection = async (rect) => {
    const selection = viewportRect(rect);
    const viewerCapture = await captureFromViewerFrame(selection);
    if (viewerCapture?.dataUrl) return viewerCapture;
    const canvasCapture = captureFromCanvasCandidates(selection);
    if (canvasCapture?.dataUrl) return canvasCapture;
    return viewerCapture?.note ? viewerCapture : canvasCapture;
  };

  const renderPreview = (capture) => {
    preview.replaceChildren();
    if (capture.dataUrl) {
      const image = document.createElement("img");
      image.alt = "Captured annotation region";
      image.src = capture.dataUrl;
      preview.append(image);
      return;
    }
    const message = document.createElement("span");
    message.className = "muted";
    message.textContent = capture.note || "Region selected.";
    preview.append(message);
  };

  const showSelection = async (rect) => {
    const section = activeSection();
    const sectionRect = section?.getBoundingClientRect() || document.body.getBoundingClientRect();
    const normalizedRect = normalizeRect(rect, sectionRect);
    const selection = {
      rect: normalizedRect,
      viewportRect: {
        left: Math.round(rect.left),
        top: Math.round(rect.top),
        width: Math.round(rect.width),
        height: Math.round(rect.height),
      },
      capture: { note: "Capturing region..." },
      tab: activeTab(),
      revision: sourceRevision(),
    };
    state.selection = selection;
    titleInput.value = `${selection.tab} annotation`;
    noteInput.value = "";
    renderPreview(selection.capture);
    drawer.classList.remove("hidden");
    setStatus("Capturing selected region...");
    const capture = await captureSelection(rect);
    if (state.selection !== selection) return;
    selection.capture = capture;
    renderPreview(capture);
    setStatus(capture.dataUrl ? "Region selected. Add a note, then submit." : capture.note);
  };

  let start;
  overlay.addEventListener("pointerdown", (event) => {
    if (!state.armed) return;
    event.preventDefault();
    start = { x: event.clientX, y: event.clientY };
    setRectEl({ left: start.x, top: start.y, width: 1, height: 1 });
    try {
      overlay.setPointerCapture?.(event.pointerId);
    } catch (_) {
      // Synthetic tests and a few browser edge cases can report a pointer id
      // the overlay cannot capture. Drag tracking still works without capture.
    }
  });
  overlay.addEventListener("pointermove", (event) => {
    if (!start) return;
    setRectEl(rectFromPoints(start, { x: event.clientX, y: event.clientY }));
  });
  overlay.addEventListener("pointerup", (event) => {
    if (!start) return;
    const rect = rectFromPoints(start, { x: event.clientX, y: event.clientY });
    start = undefined;
    setArmed(false);
    void showSelection(rect);
  });
  overlay.addEventListener("pointercancel", () => {
    start = undefined;
    setArmed(false);
  });

  const startSelection = () => setArmed(true);

  button.addEventListener("click", () => setArmed(!state.armed));
  preview.addEventListener("click", () => {
    if (!state.selection?.capture?.dataUrl) startSelection();
  });

  const renderStack = () => {
    stackEl.innerHTML = "";
    state.stack.forEach((item, index) => {
      const card = document.createElement("article");
      card.className = "annotation-card";
      card.innerHTML = `<div class="annotation-card-title"><strong></strong><button class="annotation-remove" type="button">Remove</button></div><div class="annotation-card-body"></div>`;
      card.querySelector("strong").textContent = `${index + 1}. ${item.title}`;
      card.querySelector(".annotation-card-body").textContent = item.body || "(capture only)";
      card.querySelector(".annotation-remove").addEventListener("click", () => {
        state.stack.splice(index, 1);
        renderStack();
      });
      stackEl.append(card);
    });
    addButton.disabled = state.sending;
    if (!state.sending) {
      setStatus(state.stack.length ? `${state.stack.length} queued annotation${state.stack.length === 1 ? "" : "s"}.` : "No queued annotations.");
    }
  };

  const addCurrentSelection = () => {
    const selection = state.selection;
    const body = noteInput.value.trim();
    const image = selection?.capture?.dataUrl ? { dataUrl: selection.capture.dataUrl } : undefined;
    if (!body && !image) {
      setStatus("Add a note, record a voice note, or select an area before submitting.");
      return;
    }
    const tab = selection?.tab || activeTab();
    const title = titleInput.value.trim() || (selection ? `${tab} annotation` : `${tab} note`);
    const context = {
      plugin: "kicad-pcb",
      project: window.KicadWorkbenchState?.projectId,
      tab,
      revision: selection?.revision || sourceRevision(),
      page: location.href,
      createdAt: new Date().toISOString(),
    };
    if (selection) {
      context.rect = selection.rect;
      context.viewportRect = selection.viewportRect;
      context.captureNote = selection.capture?.note;
    }
    state.stack.push({
      title,
      body,
      context,
      image,
    });
    state.selection = undefined;
    preview.innerHTML = `<span class="muted">${emptyPreviewText}</span>`;
    titleInput.value = "";
    noteInput.value = "";
    renderStack();
    void sendStack();
  };

  const sendStack = async () => {
    if (!state.stack.length || state.sending) return;
    const payload = {
      source: {
        plugin: "kicad-pcb",
        project: window.KicadWorkbenchState?.projectId,
        session: window.KicadWorkbenchState?.session,
        revision: sourceRevision(),
        page: location.href,
      },
      items: state.stack,
    };
    state.sending = true;
    renderStack();
    setStatus("Sending annotation to Agent Portal...");
    try {
      const response = await fetch(EDIT_STACK_ENDPOINT, {
        method: "POST",
        headers: { "content-type": "application/json" },
        credentials: "same-origin",
        body: JSON.stringify(payload),
      });
      if (!response.ok) {
        const message = await response.text().catch(() => "");
        throw new Error(message || `HTTP ${response.status}`);
      }
      state.stack = [];
      renderStack();
      setStatus("Sent annotation to Agent Portal.");
    } catch (err) {
      setStatus(`Could not send annotation: ${err?.message || err}`);
    } finally {
      state.sending = false;
      addButton.disabled = false;
    }
  };

  const toggleSpeech = () => {
    if (!speechCtor) return;
    if (state.recognizing) {
      state.recognition?.stop();
      return;
    }
    const recognition = new speechCtor();
    recognition.lang = document.documentElement.lang || navigator.language || "en-US";
    recognition.interimResults = true;
    recognition.continuous = true;
    let finalText = noteInput.value.trim();
    recognition.onstart = () => {
      state.recognizing = true;
      recordButton.textContent = "Stop";
      setStatus("Listening...");
    };
    recognition.onresult = (event) => {
      let interim = "";
      for (let i = event.resultIndex; i < event.results.length; i += 1) {
        const text = event.results[i][0]?.transcript || "";
        if (event.results[i].isFinal) finalText = [finalText, text.trim()].filter(Boolean).join("\n");
        else interim += text;
      }
      noteInput.value = [finalText, interim.trim()].filter(Boolean).join("\n");
    };
    recognition.onerror = (event) => setStatus(`Speech recognition failed: ${event.error || "unknown error"}`);
    recognition.onend = () => {
      state.recognizing = false;
      recordButton.textContent = "Record";
      setStatus("Speech recognition stopped.");
    };
    state.recognition = recognition;
    recognition.start();
  };

  drawer.addEventListener("click", (event) => {
    const action = event.target?.dataset?.action;
    if (action === "close") drawer.classList.add("hidden");
    if (action === "select") startSelection();
    if (action === "add") addCurrentSelection();
    if (action === "clear") {
      state.stack = [];
      renderStack();
    }
    if (action === "record") toggleSpeech();
  });

  renderStack();
})();
