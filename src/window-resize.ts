import { getCurrentWindow } from "@tauri-apps/api/window";
import { PhysicalPosition, PhysicalSize } from "@tauri-apps/api/dpi";
import { macDesktop } from "./window-drag";

/** Overlay titlebars keep native borders; inner edge grips also accept pointer/assistive input. */
export function bindWindowResize(
  host: HTMLElement,
  error: (e: unknown) => void,
) {
  if (!macDesktop) return; // Windows/Linux use their native frame/compositor resizing.
  const window = getCurrentWindow();
  for (const edge of ["n", "ne", "e", "se", "s", "sw", "w", "nw"]) {
    const grip = document.createElement("div");
    grip.className = `window-resize window-resize-${edge}`;
    grip.dataset.windowResize = edge;
    grip.setAttribute("aria-hidden", "true");
    let gesture: {
      x: number;
      y: number;
      position?: PhysicalPosition;
      size?: PhysicalSize;
      scale?: number;
    } | null = null;
    let dx = 0,
      dy = 0,
      queued = false;
    let updates = Promise.resolve();
    const apply = (
      g: NonNullable<typeof gesture>,
      x: number,
      y: number,
    ): Promise<void> => {
      updates = updates
        .catch(() => {})
        .then(async () => {
          if (!g.position || !g.size || !g.scale) return;
          const width = Math.max(
            Math.ceil(740 * g.scale),
            Math.round(
              g.size.width +
                (edge.includes("e") ? x : edge.includes("w") ? -x : 0) *
                  g.scale,
            ),
          );
          const height = Math.max(
            Math.ceil(560 * g.scale),
            Math.round(
              g.size.height +
                (edge.includes("s") ? y : edge.includes("n") ? -y : 0) *
                  g.scale,
            ),
          );
          await window.setSize(new PhysicalSize(width, height));
          if (edge.includes("w") || edge.includes("n")) {
            await window.setPosition(
              new PhysicalPosition(
                g.position.x + (edge.includes("w") ? g.size.width - width : 0),
                g.position.y +
                  (edge.includes("n") ? g.size.height - height : 0),
              ),
            );
          }
        });
      return updates;
    };
    const queue = () => {
      if (queued || !gesture?.size) return;
      queued = true;
      requestAnimationFrame(async () => {
        const g = gesture,
          x = dx,
          y = dy;
        try {
          if (g) await apply(g, x, y);
        } catch (e) {
          error(e);
        } finally {
          queued = false;
          if (gesture && (dx !== x || dy !== y)) queue();
        }
      });
    };
    grip.addEventListener("pointerdown", (event) => {
      if (event.button > 0) return;
      event.preventDefault();
      event.stopPropagation();
      const g = { x: event.screenX, y: event.screenY } as NonNullable<
        typeof gesture
      >;
      gesture = g;
      dx = dy = 0;
      try {
        grip.setPointerCapture(event.pointerId);
      } catch {
        /* Assistive pointer. */
      }
      Promise.all([
        window.outerPosition(),
        window.innerSize(),
        window.scaleFactor(),
      ])
        .then(([position, size, scale]) => {
          if (gesture !== g) return;
          Object.assign(g, { position, size, scale });
          if (dx || dy) queue();
        })
        .catch(error);
    });
    grip.addEventListener("pointermove", (event) => {
      if (!gesture) return;
      dx = event.screenX - gesture.x;
      dy = event.screenY - gesture.y;
      queue();
    });
    grip.addEventListener("pointerup", (event) => {
      const g = gesture;
      gesture = null;
      if (g && (dx || dy)) apply(g, dx, dy).catch(error);
      if (grip.hasPointerCapture(event.pointerId))
        grip.releasePointerCapture(event.pointerId);
    });
    grip.addEventListener("pointercancel", () => {
      gesture = null;
    });
    host.append(grip);
  }
}
