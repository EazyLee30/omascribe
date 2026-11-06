(() => {
  const request = __PROBE_REQUEST__;
  const invoke = window.__TAURI_INTERNALS__.invoke;
  try {
    if (!window.__omascribeTestErrors) {
      window.__omascribeTestErrors = [];
      window.addEventListener("error", (event) => window.__omascribeTestErrors.push(String(event.message)));
      window.addEventListener("unhandledrejection", (event) => window.__omascribeTestErrors.push(String(event.reason)));
    }
    const pet = document.querySelector("#pet");
    if (request.expand && pet?.getAttribute("aria-expanded") !== "true") pet?.click();
    const snapshot = {
      lang: document.documentElement.lang,
      expanded: pet?.getAttribute("aria-expanded") ?? null,
      start: document.querySelector("#pet-start")?.textContent?.trim() ?? null,
      captionLabel: document.querySelector('[data-action="captions"]')?.getAttribute("aria-label") ?? null,
      errors: window.__omascribeTestErrors.slice(-10),
    };
    void invoke("ui_test_window_result", { requestId: request.id, snapshot });
  } catch (error) {
    void invoke("ui_test_window_result", { requestId: request.id, snapshot: { error: String(error) } });
  }
})();
