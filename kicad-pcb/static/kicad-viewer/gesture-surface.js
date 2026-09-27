export function prepareGestureSurface(element) {
  element.style.touchAction = "none";
  element.style.userSelect = "none";
  element.style.webkitUserSelect = "none";
  element.style.webkitTouchCallout = "none";
  element.style.webkitTapHighlightColor = "transparent";
  element.draggable = false;
  element.addEventListener("selectstart", prevent, { capture: true });
  element.addEventListener("dragstart", prevent, { capture: true });
}

function prevent(event) {
  event.preventDefault();
}

export function pointFromPointer(event) {
  return { x: event.clientX, y: event.clientY };
}

export function centerOf(points) {
  return {
    x: (points[0].x + points[1].x) / 2,
    y: (points[0].y + points[1].y) / 2,
  };
}

export function distanceOf(points) {
  return Math.hypot(points[0].x - points[1].x, points[0].y - points[1].y);
}

export function createPointerGestureTracker(element, handlers = {}) {
  const pointers = new Map();
  let twoFinger = false;

  const capture = (event) => {
    try {
      element.setPointerCapture?.(event.pointerId);
    } catch {
      // Safari can throw when the pointer was already canceled.
    }
  };
  const release = (id) => {
    try {
      element.releasePointerCapture?.(id);
    } catch {
      // Best-effort only; state clearing below is the important bit.
    }
  };
  const stop = (event) => {
    event.preventDefault();
    event.stopImmediatePropagation();
  };
  const clear = (event) => {
    for (const id of pointers.keys()) release(id);
    pointers.clear();
    twoFinger = false;
    handlers.onClear?.(event);
  };
  const twoPoints = () => [...pointers.values()].slice(0, 2);
  const down = (event) => {
    if (event.pointerType !== "touch" || handlers.enabled?.() === false) return;
    pointers.set(event.pointerId, pointFromPointer(event));
    if (pointers.size >= 2) {
      for (const id of pointers.keys()) capture({ pointerId: id });
      twoFinger = true;
      stop(event);
      handlers.onTwoStart?.(twoPoints(), event);
    } else {
      handlers.onSingleStart?.(event);
    }
  };
  const move = (event) => {
    if (event.pointerType && event.pointerType !== "touch") return;
    if (!pointers.has(event.pointerId)) return;
    pointers.set(event.pointerId, pointFromPointer(event));
    if (!twoFinger) {
      handlers.onSingleMove?.(event);
      return;
    }
    stop(event);
    handlers.onTwoMove?.(twoPoints(), event);
  };
  const up = (event) => {
    if (event.pointerType && event.pointerType !== "touch") return;
    if (!pointers.has(event.pointerId)) return;
    if (twoFinger) stop(event);
    pointers.delete(event.pointerId);
    release(event.pointerId);
    if (twoFinger || pointers.size === 0) clear(event);
    else handlers.onSingleStart?.(event);
  };

  element.addEventListener("pointerdown", down, { capture: true });
  element.addEventListener("pointermove", move, { capture: true });
  element.addEventListener("pointerup", up, { capture: true });
  element.addEventListener("pointercancel", up, { capture: true });
  element.addEventListener("lostpointercapture", up, { capture: true });

  return {
    clear,
    dispose() {
      clear();
      element.removeEventListener("pointerdown", down, { capture: true });
      element.removeEventListener("pointermove", move, { capture: true });
      element.removeEventListener("pointerup", up, { capture: true });
      element.removeEventListener("pointercancel", up, { capture: true });
      element.removeEventListener("lostpointercapture", up, { capture: true });
    },
  };
}
