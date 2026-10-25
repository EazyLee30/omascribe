import "@awesome.me/webawesome/dist/components/button/button.js";
import "@awesome.me/webawesome/dist/components/select/select.js";
import "@awesome.me/webawesome/dist/components/option/option.js";
import "@awesome.me/webawesome/dist/components/input/input.js";
import "@awesome.me/webawesome/dist/components/textarea/textarea.js";
import "@awesome.me/webawesome/dist/components/dialog/dialog.js";
import "@awesome.me/webawesome/dist/components/drawer/drawer.js";
import "@awesome.me/webawesome/dist/components/details/details.js";
import "@awesome.me/webawesome/dist/components/tab-group/tab-group.js";
import "@awesome.me/webawesome/dist/translations/zh-cn.js";
import "@awesome.me/webawesome/dist/translations/zh-tw.js";
import { registerIconLibrary } from "@awesome.me/webawesome/dist/components/icon/library.js";

// Internal component icons are copied from the pinned package during the build.
// Both libraries resolve locally; an accidental default icon cannot call a CDN.
const localIcons = {
  resolver: (name, _family, variant = "solid") =>
    `/assets/ui-icons/${variant}/${name}.svg`,
  mutator: (svg) => svg.setAttribute("fill", "currentColor"),
};
registerIconLibrary("system", localIcons);
registerIconLibrary("default", localIcons);

await import("../public/site.js");

const lang = document.documentElement.dataset.language || "zh";
const t = (zh, tw, en) => ({ zh, tw, en })[lang];
const drawer = document.querySelector("#mobile-nav");
document.querySelector("#menu-toggle")?.addEventListener("click", () => {
  drawer.open = true;
});
drawer?.querySelectorAll("a").forEach((a) =>
  a.addEventListener("click", () => {
    drawer.open = false;
  }),
);

const platformTabs = document.querySelector("#platform-tabs");
function selectPlatformFromLink() {
  const platform = location.hash.slice(1);
  if (platformTabs && ["mac", "windows", "linux"].includes(platform)) platformTabs.active = platform;
}
selectPlatformFromLink();
window.addEventListener("hashchange", selectPlatformFromLink);

const notes = document.querySelector("#demo-notes");
if (notes) {
  const initial = notes.value;
  const status = document.querySelector("#demo-status");
  document.querySelector("#demo-source").addEventListener("click", async () => {
    const tabs = document.querySelector("#demo-tabs");
    tabs.active = "transcript";
    await tabs.updateComplete;
    const row = document.querySelector("#demo-0034");
    row.classList.add("source-highlight");
    row.focus({ preventScroll: true });
    row.scrollIntoView({ block: "nearest" });
  });
  document.querySelector("#demo-export").addEventListener("click", () => {
    const blob = new Blob([notes.value + "\n"], {
      type: "text/markdown;charset=utf-8",
    });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = "omascribe-demo-notes.md";
    a.click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
    status.textContent = t(
      "已生成当前笔记的 Markdown 文件。",
      "已產生目前筆記的 Markdown 檔案。",
      "Markdown generated from your current notes.",
    );
  });
  const resetDialog = document.querySelector("#reset-demo");
  const reset = () => {
    notes.value = initial;
    status.textContent = t("已恢复示例。", "已還原範例。", "Sample restored.");
    resetDialog.open = false;
  };
  document.querySelector("#demo-reset").addEventListener("click", () => {
    if (notes.value !== initial) resetDialog.open = true;
    else reset();
  });
  document.querySelector("#cancel-reset").addEventListener("click", () => {
    resetDialog.open = false;
  });
  document.querySelector("#confirm-reset").addEventListener("click", reset);
}
