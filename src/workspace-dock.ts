import {
  createDockview,
  themeLight,
  type DockviewApi,
  type SerializedDockview,
} from "dockview";
import "dockview/dist/styles/dockview.css";
import { esc, tr } from "./workspace-ui";
import { computePosition, flip, offset, shift } from "@floating-ui/dom";
export type DockPanel = "notebook" | "minutes" | "transcript";
const keys: DockPanel[] = ["notebook", "minutes", "transcript"];
const title = (id: DockPanel) =>
  ({
    notebook: tr("我的笔记", "Your notes", "我的筆記"),
    minutes: tr("AI 会议纪要", "AI minutes", "AI 會議紀要"),
    transcript: tr("实时转写", "Live transcript", "即時轉寫"),
  })[id];
const storageKey = "omascribe-workspace-v1";
interface SavedLayout {
  id: string;
  name: string;
  layout: SerializedDockview;
}
interface SavedState {
  current?: SerializedDockview;
  layouts: SavedLayout[];
  defaultId?: string;
}
function read(): SavedState {
  try {
    const s = JSON.parse(localStorage.getItem(storageKey) || "{}");
    return {
      current: s.current,
      layouts: Array.isArray(s.layouts) ? s.layouts.slice(0, 12) : [],
      defaultId: s.defaultId,
    };
  } catch {
    return { layouts: [] };
  }
}
export class WorkspaceDock {
  readonly api: DockviewApi;
  private saving = false;
  private timer: ReturnType<typeof setTimeout> | undefined;
  private observer: ResizeObserver;
  private menu: HTMLDivElement;
  private disposed = false;
  private menuTrigger: HTMLElement | undefined;
  constructor(
    private host: HTMLElement,
    private panels: Record<DockPanel, HTMLElement>,
    private parking: HTMLElement,
    private changed: (visible: Record<DockPanel, boolean>) => void,
  ) {
    this.menu = document.createElement("div");
    this.menu.className = "dock-panel-menu";
    this.menu.hidden = true;
    this.menu.setAttribute("role", "menu");
    document.body.append(this.menu);
    this.menu.addEventListener("keydown", (e) => {
      if (e.key === "Escape") {
        this.menu.hidden = true;
        this.menuTrigger?.focus();
      } else if (["ArrowDown", "ArrowUp", "Home", "End"].includes(e.key)) {
        e.preventDefault();
        const items = Array.from(
          this.menu.querySelectorAll<HTMLButtonElement>("button"),
        );
        const index = items.indexOf(
          document.activeElement as HTMLButtonElement,
        );
        items[
          e.key === "Home"
            ? 0
            : e.key === "End"
              ? items.length - 1
              : (index + (e.key === "ArrowDown" ? 1 : -1) + items.length) %
                items.length
        ]?.focus();
      }
    });
    this.api = createDockview(host, {
      theme: {
        ...themeLight,
        className: "dockview-theme-light oma-dock-theme",
      },
      floatingGroupBounds: "boundedWithinViewport",
      floatingGroupDragHandle: "tabbar",
      singleTabMode: "default",
      defaultTabComponent: "column-title",
      createTabComponent: () => {
        const element = document.createElement("div");
        element.className = "oma-panel-tab";
        let disposeTitle: { dispose(): void } | undefined;
        return {
          element,
          init(params) {
            const render = (value: string) => {
              element.textContent = value;
              element.title = tr(
                "拖动调整栏目位置",
                "Drag to rearrange columns",
                "拖曳調整欄位位置",
              );
            };
            render(params.title);
            disposeTitle = params.api.onDidTitleChange(({ title }) =>
              render(title),
            );
          },
          dispose() {
            disposeTitle?.dispose();
          },
        };
      },
      dndStrategy: "pointer",
      transformFloatingGroupDrag: ({ proposed, container }) => ({
        left:
          Math.abs(proposed.left) < 18
            ? 0
            : Math.abs(proposed.left + container.width - host.clientWidth) < 18
              ? host.clientWidth - container.width
              : proposed.left,
        top: Math.abs(proposed.top) < 18 ? 0 : proposed.top,
      }),
      createComponent: ({ id }) => {
        if (!keys.includes(id as DockPanel))
          throw Error("Unknown workspace panel");
        const content = panels[id as DockPanel];
        const element = document.createElement("div");
        element.className = "dock-panel-content";
        return {
          element,
          init() {
            content.hidden = false;
            element.append(content);
          },
          dispose() {
            if (element.contains(content)) parking.append(content);
          },
        };
      },
      createRightHeaderActionComponent: (group) => {
        const element = document.createElement("button");
        element.className = "dock-group-menu";
        element.textContent = "⋯";
        element.title = tr(
          "面板位置与显示",
          "Panel position and visibility",
          "面板位置與顯示",
        );
        element.setAttribute("aria-label", element.title);
        element.setAttribute("aria-haspopup", "menu");
        element.onclick = () => {
          const panel = group.activePanel;
          if (panel) this.panelMenu(panel.id as DockPanel, element);
        };
        return {
          element,
          init() {},
          dispose() {
            element.remove();
          },
        };
      },
      createWatermarkComponent: () => {
        const element = document.createElement("div");
        element.className = "dock-empty";
        element.innerHTML = `<p>${tr("选择一个面板开始", "Choose a panel to begin", "選擇面板開始")}</p><button data-dock-restore>${tr("恢复默认布局", "Restore default layout", "恢復預設佈局")}</button>`;
        element.querySelector("button")!.onclick = () => this.preset("default");
        return { element, init() {} };
      },
    });
    const saved = read();
    if (saved.current) {
      try {
        this.load(saved.current);
      } catch {
        this.preset("default");
      }
    } else {
      const preferred = saved.layouts.find((l) => l.id === saved.defaultId);
      if (preferred) {
        try {
          this.load(preferred.layout);
        } catch {
          this.preset("default");
        }
      } else this.preset("default");
    }
    this.api.onDidLayoutChange(() => {
      clearTimeout(this.timer);
      this.timer = setTimeout(() => {
        if (this.disposed) return;
        const s = read();
        s.current = this.api.toJSON();
        localStorage.setItem(storageKey, JSON.stringify(s));
        this.notify();
      }, 180);
    });
    this.observer = new ResizeObserver(() =>
      this.api.layout(host.clientWidth, host.clientHeight),
    );
    this.observer.observe(host);
    document.addEventListener("pointerdown", this.outside);
    document.addEventListener("keydown", this.escape);
    queueMicrotask(() => this.notify());
  }
  private outside = (e: Event) => {
    if (
      !this.menu.contains(e.target as Node) &&
      !(e.target as HTMLElement).closest(".dock-group-menu")
    )
      this.menu.hidden = true;
  };
  private escape = (e: KeyboardEvent) => {
    if (e.key === "Escape") this.menu.hidden = true;
  };
  private notify() {
    if (!this.saving)
      this.changed(
        Object.fromEntries(
          keys.map((id) => [id, !!this.api.getPanel(id)]),
        ) as Record<DockPanel, boolean>,
      );
  }
  has(id: DockPanel) {
    return !!this.api.getPanel(id);
  }
  show(id: DockPanel, floating = false) {
    let p = this.api.getPanel(id);
    if (!p) {
      p = this.api.addPanel({
        id,
        component: id,
        title: title(id),
        renderer: "always",
        initialWidth: Math.max(280, Math.round(this.host.clientWidth * 0.28)),
        minimumWidth: 220,
        minimumHeight: 180,
        position: this.api.panels.length
          ? {
              referencePanel:
                this.api.getPanel("minutes") ||
                this.api.panels[this.api.panels.length - 1],
              direction: this.host.clientWidth < 960 ? "within" : "right",
            }
          : undefined,
      });
      if (!floating) this.balance();
    }
    if (floating)
      this.api.addFloatingGroup(p, {
        x: Math.max(0, this.host.clientWidth - 390),
        y: Math.max(0, this.host.clientHeight - 520),
        width: Math.min(390, this.host.clientWidth),
        height: Math.min(440, this.host.clientHeight - 70),
      });
    p.api.setActive();
    this.notify();
  }
  private balance() {
    const panels = keys.map((id) => this.api.getPanel(id)).filter((p) => !!p);
    if (
      panels.length < 2 ||
      new Set(panels.map((p) => p.group.id)).size !== panels.length ||
      panels.some((p) => p.api.location.type !== "grid")
    )
      return;
    const grid = this.api.toJSON().grid;
    // Preserve stacked and nested custom layouts; balance only a simple row.
    if (
      grid.orientation !== "HORIZONTAL" ||
      !Array.isArray(grid.root.data) ||
      grid.root.data.some((node) => node.type !== "leaf")
    )
      return;
    const width = this.host.clientWidth;
    const ratios = panels.length === 3 ? [0.36, 0.38, 0.26] : [0.5, 0.5];
    panels.forEach((p, i) =>
      p.api.setSize({ width: Math.round(width * ratios[i]) }),
    );
  }
  hide(id: DockPanel) {
    const p = this.api.getPanel(id);
    if (p) this.api.removePanel(p);
    this.notify();
  }
  preset(name: string) {
    const preferred = read().layouts.find((l) => l.id === read().defaultId);
    if (name === "default" && preferred) {
      try {
        this.load(preferred.layout);
        this.notify();
        return;
      } catch {
        const s = read();
        delete s.defaultId;
        localStorage.setItem(storageKey, JSON.stringify(s));
      }
    }
    this.saving = true;
    this.api.clear();
    if (name === "notes") this.show("notebook");
    else if (name === "minutes") this.show("minutes");
    else {
      this.show("notebook");
      this.api.addPanel({
        id: "minutes",
        component: "minutes",
        title: title("minutes"),
        renderer: "always",
        minimumWidth: 220,
        minimumHeight: 180,
        position: {
          referencePanel: "notebook",
          direction: this.host.clientWidth < 720 ? "within" : "right",
        },
      });
      if (name === "meeting") this.show("transcript");
    }
    this.saving = false;
    this.notify();
  }
  private load(layout: SerializedDockview) {
    if (
      JSON.stringify(layout).length > 100000 ||
      Object.keys(layout.panels).some((id) => !keys.includes(id as DockPanel))
    )
      throw Error("Invalid local layout");
    for (const [id, p] of Object.entries(layout.panels)) {
      if (p.contentComponent !== id) throw Error("Invalid panel component");
      p.title = title(id as DockPanel);
    }
    this.saving = true;
    try {
      this.api.fromJSON(layout);
    } finally {
      this.saving = false;
    }
  }
  saved() {
    return read().layouts.map((l) => ({
      id: l.id,
      name: l.name,
      default: read().defaultId === l.id,
    }));
  }
  save(name: string) {
    const s = read();
    const id = crypto.randomUUID();
    s.layouts.push({
      id,
      name: name.trim().slice(0, 40) || tr("我的布局", "My layout", "我的佈局"),
      layout: this.api.toJSON(),
    });
    s.layouts = s.layouts.slice(-12);
    localStorage.setItem(storageKey, JSON.stringify(s));
    return id;
  }
  open(id: string) {
    const l = read().layouts.find((l) => l.id === id);
    if (l) {
      this.load(l.layout);
      this.notify();
    }
  }
  makeDefault(id: string) {
    const s = read();
    s.defaultId = id;
    localStorage.setItem(storageKey, JSON.stringify(s));
  }
  remove(id: string) {
    const s = read();
    s.layouts = s.layouts.filter((l) => l.id !== id);
    if (s.defaultId === id) delete s.defaultId;
    localStorage.setItem(storageKey, JSON.stringify(s));
  }
  private panelMenu(id: DockPanel, trigger: HTMLElement) {
    const p = this.api.getPanel(id);
    if (!p) return;
    this.menu.innerHTML = `<small>${esc(title(id))}</small>${[
      ["left", tr("停靠左侧", "Dock left", "停靠左側")],
      ["right", tr("停靠右侧", "Dock right", "停靠右側")],
      ["bottom", tr("停靠底部", "Dock bottom", "停靠底部")],
      ["center", tr("合并为标签页", "Merge into tabs", "合併為分頁")],
      ["float", tr("窗口内浮动", "Float inside window", "視窗內浮動")],
      ["focus", tr("最大化 / 还原", "Maximize / restore", "最大化 / 還原")],
      ["balance", tr("均衡列宽", "Balance column widths", "均衡欄寬")],
      ["hide", tr("隐藏此栏", "Hide this column", "隱藏此欄")],
    ]
      .map(
        ([action, label]) =>
          `<button role="menuitem" data-place="${action}">${label}</button>`,
      )
      .join(
        "",
      )}<div class="dock-menu-hint">${tr("隐藏后可从「视图」恢复，内容保留。", "Restore hidden columns from View. Content is kept.", "隱藏後可從「檢視」恢復，內容保留。")}</div>`;
    this.menuTrigger = trigger;
    this.menu.hidden = false;
    void computePosition(trigger, this.menu, {
      strategy: "fixed",
      placement: "bottom-end",
      middleware: [offset(6), flip(), shift({ padding: 8 })],
    }).then(({ x, y }) => {
      if (!this.menu.hidden)
        Object.assign(this.menu.style, { left: `${x}px`, top: `${y}px` });
    });
    this.menu.querySelector<HTMLButtonElement>("button")?.focus();
    this.menu.onclick = (e) => {
      const action = (e.target as HTMLElement).closest<HTMLElement>(
        "[data-place]",
      )?.dataset.place;
      if (!action) return;
      this.menu.hidden = true;
      if (action === "float") this.show(id, true);
      else if (action === "hide") this.hide(id);
      else if (action === "balance") this.balance();
      else if (action === "focus") {
        if (p.api.isMaximized()) p.api.exitMaximized();
        else p.api.maximize();
      } else {
        const other = this.api.panels.find((o) => o.id !== id);
        if (other)
          p.api.moveTo({
            group: other.group,
            position: action as "left" | "right" | "bottom" | "center",
          });
      }
      this.notify();
    };
  }
  dispose() {
    this.disposed = true;
    clearTimeout(this.timer);
    this.observer.disconnect();
    document.removeEventListener("pointerdown", this.outside);
    document.removeEventListener("keydown", this.escape);
    this.api.dispose();
    this.menu.remove();
  }
}
