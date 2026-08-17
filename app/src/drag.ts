/**
 * Drag a dialog by its header. Position is remembered per `id` (localStorage) and clamped to
 * the viewport, so a palette can be moved off the text it would otherwise cover.
 */
export function makeDraggable(dialog: HTMLElement, handle: HTMLElement, id: string) {
  const key = `kansa.dialog.${id}`;
  let dx = 0, dy = 0; // translation applied to the dialog
  const apply = () => { dialog.style.transform = dx || dy ? `translate(${dx}px, ${dy}px)` : ""; };
  const clamp = () => {
    const r = dialog.getBoundingClientRect();
    const vw = window.innerWidth, vh = window.innerHeight;
    // keep at least the header (40px) reachable inside the viewport
    if (r.left < 8 - r.width + 120) dx += 8 - r.width + 120 - r.left;
    if (r.right > vw + r.width - 120) dx -= r.right - (vw + r.width - 120);
    if (r.top < 0) dy -= r.top;
    if (r.top > vh - 40) dy -= r.top - (vh - 40);
    apply();
  };
  try {
    const saved = JSON.parse(localStorage.getItem(key) ?? "null");
    if (saved && typeof saved.dx === "number") { dx = saved.dx; dy = saved.dy; apply(); requestAnimationFrame(clamp); }
  } catch { /* ignore */ }

  handle.style.cursor = "grab";
  handle.style.userSelect = "none";
  handle.addEventListener("pointerdown", (e) => {
    if ((e.target as HTMLElement).closest("button, input, textarea, select, a")) return;
    e.preventDefault();
    handle.setPointerCapture(e.pointerId);
    handle.style.cursor = "grabbing";
    const sx = e.clientX - dx, sy = e.clientY - dy;
    const move = (ev: PointerEvent) => { dx = ev.clientX - sx; dy = ev.clientY - sy; apply(); };
    const up = () => {
      handle.removeEventListener("pointermove", move);
      handle.removeEventListener("pointerup", up);
      handle.style.cursor = "grab";
      clamp();
      try { localStorage.setItem(key, JSON.stringify({ dx, dy })); } catch { /* ignore */ }
    };
    handle.addEventListener("pointermove", move);
    handle.addEventListener("pointerup", up);
  });
  // double-click the header to snap back to the default position
  handle.addEventListener("dblclick", () => { dx = 0; dy = 0; apply(); localStorage.removeItem(key); });
}
