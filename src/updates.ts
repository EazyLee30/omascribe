import { invoke } from "@tauri-apps/api/core";
import type { NetworkPolicy } from "./preferences-client";
import { bytes, esc, tr } from "./workspace-ui";
interface UpdateStatus {
  phase: string;
  version: string | null;
  notes: string | null;
  downloaded: number;
  total: number | null;
  error: string | null;
  supported: boolean;
}
export async function mountUpdates(
  root: HTMLElement,
  version: string,
  toast: (message: string, error?: boolean) => void,
) {
  let alive = true;
  let working = false;
  let signature = "";
  const errors: Record<string, string> = {
    "update:network-disabled": tr(
      "请先允许软件更新联网。",
      "Allow software update connections first.",
      "請先允許軟體更新連網。",
    ),
    "update:finish-work": tr(
      "请先结束录音，并等待转写、纪要和说话人分析完成。",
      "Finish recording and wait for transcription, minutes and speaker analysis to complete.",
      "請先結束錄音，並等待轉寫、紀要及說話人分析完成。",
    ),
    "update:cancelled": tr(
      "已取消下载或检查。",
      "Download or check cancelled.",
      "已取消下載或檢查。",
    ),
    "update:package-manager": tr(
      "此安装方式请通过系统包管理器升级。",
      "Update this installation with your system package manager.",
      "此安裝方式請透過系統套件管理器升級。",
    ),
    "update:untrusted-release": tr(
      "更新来源不符合要求，已阻止下载。",
      "The update source was rejected.",
      "更新來源不符要求，已阻止下載。",
    ),
  };
  async function refresh() {
    const [s, policy] = await Promise.all([
      invoke<UpdateStatus>("update_status"),
      invoke<NetworkPolicy>("get_network_policy"),
    ]);
    const next = JSON.stringify([s, policy]);
    if (!alive || next === signature) return;
    signature = next;
    const busy = [
      "checking",
      "downloading",
      "verifying",
      "installing",
    ].includes(s.phase);
    const labels: Record<string, string> = {
      checking: tr("正在检查…", "Checking…", "正在檢查…"),
      current: tr("已是最新版本", "You're up to date", "已是最新版本"),
      available: tr("有新版本可用", "An update is available", "有新版本可用"),
      downloading: tr("正在下载更新…", "Downloading update…", "正在下載更新…"),
      verifying: tr("正在验证签名…", "Verifying signature…", "正在驗證簽章…"),
      ready: tr(
        "已验证，可安装",
        "Verified and ready to install",
        "已驗證，可安裝",
      ),
      installing: tr(
        "正在安装，完成后重启…",
        "Installing; the app will restart…",
        "正在安裝，完成後重新啟動…",
      ),
      error: tr("更新未完成", "Update not completed", "更新未完成"),
    };
    root.innerHTML = `<header class="page-heading"><h1>${tr("软件更新", "Software updates", "軟體更新")}</h1><p>OmaScribe ${esc(version)}</p></header>
      <div class="settings-group"><div class="settings-row"><div><label for="updates-network">${tr("允许检查与下载软件更新", "Allow update checks and downloads", "允許檢查及下載軟體更新")}</label><p>${tr("仅在你操作时连接 GitHub 获取版本信息和签名安装包。GitHub 会收到普通网络连接信息；不会上传会议、录音、转写、笔记或硬件信息。", "Your actions contact GitHub for release information and signed installers. GitHub receives normal connection metadata, never meetings, audio, transcripts, notes or hardware information.", "僅在你操作時連接 GitHub 取得版本及簽章安裝包；不傳送會議、錄音、轉寫、筆記或硬體資訊。")}</p></div><input id="updates-network" type="checkbox" role="switch" ${policy.allow_update_checks ? "checked" : ""} ${s.phase === "installing" ? "disabled" : ""}></div></div>
      ${!s.supported ? `<p class="setting-help">${tr("开发版本和 Linux 系统包不使用应用内安装器。Debian：安装新版 deb。AppImage、macOS 和 Windows 正式安装包支持此处升级。", "Development builds and Linux system packages use external installers. Debian: install the new deb. AppImage, macOS and Windows release installations support in-app updates.", "開發版本及 Linux 系統套件使用外部安裝器。Debian：安裝新版 deb。AppImage、macOS 及 Windows 正式版支援此處升級。")}</p>` : ""}
      <div class="settings-group"><div class="settings-row"><div role="status"><strong>${labels[s.phase] || tr("手动检查更新", "Check for updates", "手動檢查更新")}${s.version ? ` · ${esc(s.version)}` : ""}</strong>${s.error ? `<p class="inline-error">${esc(errors[s.error] || s.error)}</p>` : ""}${s.phase === "downloading" ? `<progress ${s.total ? `max="${s.total}" value="${s.downloaded}"` : ""}></progress><p>${bytes(s.downloaded)}${s.total ? ` / ${bytes(s.total)}` : ""}</p>` : ""}</div><div class="settings-control">${busy ? (s.phase === "installing" ? "" : `<button class="outline" data-update="cancel_update">${tr("取消", "Cancel")}</button>`) : `<button class="outline" data-update="check_update" ${!policy.allow_update_checks || !s.supported ? "disabled" : ""}>${tr("检查更新", "Check for updates", "檢查更新")}</button>`}${["available", "error"].includes(s.phase) && s.version ? `<button class="primary" data-update="download_update" ${!policy.allow_update_checks ? "disabled" : ""}>${tr("下载更新", "Download update", "下載更新")}</button>` : ""}${s.phase === "ready" ? `<button class="primary" data-update="install_update" ${!policy.allow_update_checks ? "disabled" : ""}>${tr("安装并重启", "Install and restart", "安裝並重新啟動")}</button>` : ""}</div></div></div>
      ${s.notes ? `<h2 class="settings-subtitle">${tr("版本说明", "Release notes", "版本說明")}</h2><pre class="update-notes">${esc(s.notes)}</pre>` : ""}
      <p class="setting-help">${tr("日常更新只下载程序和运行库，复用本机模型；首次安装仍包含默认模型。安装前会校验并保留内置模型，不重复下载。更新须通过签名验证，录音或处理期间不能安装。0.3.2 及更早版本升级到本版仍需一次完整更新。", "Routine updates download the app and runtime, reusing local models. First-time installers include default models. Bundled models are verified and preserved before installation. Updating from 0.3.2 or earlier still requires one full update. Signed packages are required; installation waits for content processing to finish.", "日常更新僅下載程式及執行庫，重用本機模型；首次安裝仍包含預設模型。安裝前會驗證並保留內建模型。更新須通過簽章驗證，錄音或處理期間不能安裝。0.3.2 及更早版本升級至本版仍需一次完整更新。")}</p>`;
  }
  async function click(e: Event) {
    const command = (e.target as HTMLElement).closest<HTMLElement>(
      "[data-update]",
    )?.dataset.update;
    if (!command || (working && command !== "cancel_update")) return;
    working = true;
    try {
      await invoke(command);
    } catch (error) {
      toast(errors[String(error)] || String(error), true);
    } finally {
      working = false;
      if (alive) await refresh();
    }
  }
  async function change(e: Event) {
    if ((e.target as HTMLElement).id !== "updates-network") return;
    try {
      const policy = await invoke<NetworkPolicy>("get_network_policy");
      policy.allow_update_checks = (e.target as HTMLInputElement).checked;
      await invoke("set_network_policy", { policy });
      await refresh();
    } catch (e) {
      toast(String(e), true);
    }
  }
  root.addEventListener("click", click);
  root.addEventListener("change", change);
  await refresh();
  const timer = setInterval(() => {
    void refresh().catch(() => {});
  }, 500);
  return () => {
    alive = false;
    clearInterval(timer);
    root.removeEventListener("click", click);
    root.removeEventListener("change", change);
  };
}
