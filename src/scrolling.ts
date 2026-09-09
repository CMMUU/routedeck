// Do not intercept touch, horizontal gestures, zoom or native form controls.
// Small/fractional pixel deltas already carry touchpad precision and inertia.
export function coarseWheelDelta(delta: number, mode: number, pageHeight: number): number | null {
  if (!Number.isFinite(delta) || delta === 0) return null;
  if (mode === 1) return delta * 32;
  if (mode === 2) return delta * pageHeight;
  if (mode !== 0 || !Number.isInteger(delta) || Math.abs(delta) < 40) return null;
  return delta;
}

export function nextScrollTarget(current: number, pending: number | null, delta: number, maximum: number): number {
  // Reversing direction immediately discards old momentum.
  const base = pending !== null && Math.sign(pending - current) === Math.sign(delta) ? pending : current;
  return Math.max(0, Math.min(maximum, base + delta));
}

export function scrollFrame(current: number, target: number, elapsed: number): number {
  const distance = target - current;
  const step = Math.abs(distance) * (1 - Math.exp(-Math.min(64, Math.max(1, elapsed)) / 65));
  // WebViews may round scrollTop to whole pixels; always converge, never spin.
  return current + Math.sign(distance) * Math.min(Math.abs(distance), Math.max(1, step));
}

export function installContinuousScrolling(doc: Document = document) {
  const win = doc.defaultView!;
  const reduced = win.matchMedia("(prefers-reduced-motion: reduce)");
  const surfaces = ".page-scroll, .nav-list, .log-list, .node-details-modal, .table-wrap, .app-update-notes";
  let frame = 0;
  let active: { element: HTMLElement; target: number; lastTop: number; timestamp: number } | null = null;
  const cancel = () => {
    if (frame) win.cancelAnimationFrame(frame);
    frame = 0;
    active = null;
  };
  const tick = (timestamp: number) => {
    frame = 0;
    if (!active) return;
    const state = active;
    const element = state.element;
    // Navigation, modal locking, keyboard and external scroll restoration win.
    if (reduced.matches || !element.isConnected || !element.getClientRects().length
      || !/^(auto|scroll)$/.test(win.getComputedStyle(element).overflowY)
      || Math.abs(element.scrollTop - state.lastTop) > 1) { cancel(); return; }
    state.target = Math.min(state.target, Math.max(0, element.scrollHeight - element.clientHeight));
    const top = scrollFrame(element.scrollTop, state.target, timestamp - state.timestamp);
    element.scrollTo({ top, behavior: "instant" });
    state.lastTop = element.scrollTop;
    state.timestamp = timestamp;
    if (Math.abs(element.scrollTop - state.target) < 0.5) {
      element.scrollTo({ top: state.target, behavior: "instant" });
      active = null;
    } else frame = win.requestAnimationFrame(tick);
  };
  const wheel = (event: WheelEvent) => {
    if (event.defaultPrevented || !event.cancelable || reduced.matches || event.ctrlKey
      || event.metaKey || event.altKey || event.shiftKey || event.deltaX !== 0
      || !(event.target instanceof Element)
      || event.target.closest("input, textarea, select, [contenteditable]:not([contenteditable=false])")) {
      cancel(); return;
    }
    let element: HTMLElement | null = event.target.closest(surfaces);
    while (element) {
      const style = win.getComputedStyle(element);
      const maximum = element.scrollHeight - element.clientHeight;
      const delta = coarseWheelDelta(event.deltaY, event.deltaMode, element.clientHeight);
      if (delta === null) { cancel(); return; }
      const current = element.scrollTop;
      if (/^(auto|scroll)$/.test(style.overflowY) && maximum > 0
        && (delta < 0 ? current > 0 : current < maximum - 0.5)) {
        const pending = active?.element === element ? active.target : null;
        const target = nextScrollTarget(current, pending, delta, maximum);
        event.preventDefault();
        cancel();
        active = { element, target, lastTop: current, timestamp: win.performance.now() };
        frame = win.requestAnimationFrame(tick);
        return;
      }
      // Respect modal/nav containment instead of sending input into the background.
      if (style.overscrollBehaviorY === "contain" || style.overscrollBehaviorY === "none") break;
      element = element.parentElement?.closest(surfaces) ?? null;
    }
    cancel(); // At an edge, leave native chaining/containment untouched.
  };
  doc.addEventListener("wheel", wheel, { passive: false });
  doc.addEventListener("keydown", cancel, true);
  doc.addEventListener("pointerdown", cancel, true);
  doc.addEventListener("visibilitychange", cancel);
  reduced.addEventListener("change", cancel);
  return {
    cancel,
    dispose() {
      cancel();
      doc.removeEventListener("wheel", wheel);
      doc.removeEventListener("keydown", cancel, true);
      doc.removeEventListener("pointerdown", cancel, true);
      doc.removeEventListener("visibilitychange", cancel);
      reduced.removeEventListener("change", cancel);
    },
  };
}
