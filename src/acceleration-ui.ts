import type { Hardware } from "./preferences-client";
import { bytes, esc, glyph, tr } from "./workspace-ui";

export interface ComponentCatalog {
  state: string;
  verification?: {
    state: string;
    component_id: string | null;
    version: string | null;
    reason: string | null;
    report: {
      cpu: { total_ms: number; peak_rss_mib: number | null };
      gpu: {
        total_ms: number;
        peak_rss_mib: number | null;
        runtime_gpu_buffer_mib: number | null;
      };
      faster: boolean;
    } | null;
  };
  components: {
    component: {
      id: string;
      name: string;
      backend: string;
      archive: { bytes: number };
      license: string;
    };
    version: string;
    present: boolean;
    installed: boolean;
    download: {
      state: string;
      downloaded: number;
      total: number;
      error: string | null;
    } | null;
  }[];
}
export function accelerationBody(
  h: Hardware,
  catalog: ComponentCatalog,
): string {
  if (!["windows", "linux"].includes(h.platform)) return "";
  const devices = h.gpus?.devices || [];
  // Remote display sessions can expose several DXGI adapters for the same GPU
  // model. Keep raw identities in the probe/certificate, summarize models here.
  const deviceModels = devices.filter(
    (device, index) =>
      devices.findIndex(
        (d) =>
          d.name === device.name &&
          d.vendor === device.vendor &&
          d.dedicated_memory_bytes === device.dedicated_memory_bytes,
      ) === index,
  );
  const cudaVersion = h.gpus?.cuda_driver_version;
  const cudaOld = cudaVersion != null && cudaVersion < 12000;
  const cudaBlocked = cudaOld || !devices.some((d) => d.vendor === "nvidia");
  const verification = catalog.verification;
  const testing = verification?.state === "testing";
  const matchesVerification = (id: string, version: string) =>
    verification?.component_id === id && verification.version === version;
  const statusLabel = (id: string, version: string) => {
    if (!matchesVerification(id, version))
      return tr(
        "已下载 · 待验证",
        "Downloaded · test required",
        "已下載 · 待驗證",
      );
    switch (verification?.state) {
      case "active":
        return tr(
          "已验证 · 自动启用",
          "Verified · automatic",
          "已驗證 · 自動啟用",
        );
      case "testing":
        return tr("本机测试中", "Testing locally", "本機測試中");
      case "cpu_preferred":
        return tr("CPU 更快", "CPU is faster", "CPU 較快");
      case "cpu_fallback":
        return tr("已回退 CPU", "Using CPU fallback", "已回退 CPU");
      case "test_failed":
        return tr("测试未通过", "Test did not pass", "測試未通過");
      default:
        return tr("需要重新测试", "Test again", "需要重新測試");
    }
  };
  const report = verification?.report;
  const failure = verification?.reason
    ? `<p class="inline-error">${tr("显卡测试未通过，继续使用 CPU。请检查驱动后重试。", "GPU test did not pass; CPU remains active. Check the driver and try again.", "顯示卡測試未通過，繼續使用 CPU；請檢查驅動程式後重試。")}</p><details><summary>${tr("测试详情", "Test details", "測試詳情")}</summary><p class="setting-help">${esc(verification.reason)}</p></details>`
    : "";
  const comparison = report
    ? `<p class="setting-help">${tr("本机短文本测试", "Local short-text test", "本機短文測試")} · CPU ${(report.cpu.total_ms / 1000).toFixed(2)} s · GPU ${(report.gpu.total_ms / 1000).toFixed(2)} s${report.gpu.peak_rss_mib !== null ? ` · ${tr("进程内存峰值", "Peak process memory", "程序記憶體峰值")} ${report.gpu.peak_rss_mib} MiB` : ""}</p>`
    : "";
  const vendors = new Set(devices.map((d) => d.vendor));
  const recommended = (backend: string) =>
    backend === "cuda"
      ? vendors.has("nvidia") && !cudaBlocked
      : backend === "vulkan" &&
        ([...vendors].some((v) => ["amd", "intel"].includes(v)) ||
          (cudaOld && vendors.has("nvidia")));
  return `<div class="settings-section-title"><h2>${tr("显卡加速组件", "GPU acceleration", "顯示卡加速元件")}</h2><button class="outline" data-component-action="check">${tr("检查可用组件", "Check components", "檢查可用元件")}</button></div>
  <p class="setting-help">${tr("用于本地翻译与纪要，组件独立下载。通过本机模型测试且更快后，自动模式才会使用显卡。", "For local translation and minutes. Components download separately. Automatic mode uses the GPU only after a successful, faster local model test.", "元件獨立下載，不增加主安裝包體積；依顯示卡推薦，通過本機模型驗證後才啟用。")}</p>
  <div class="settings-group">${deviceModels.length ? deviceModels.map((d) => `<div class="settings-row"><div><label>${esc(d.name)}</label><p>${esc(d.vendor.toUpperCase())}${d.dedicated_memory_bytes ? ` · ${bytes(d.dedicated_memory_bytes)} ${tr("独立显存", "dedicated memory", "獨立顯示記憶體")}` : ""}</p></div><span class="badge">${tr("已检测", "Detected", "已偵測")}</span></div>`).join("") : `<p class="setting-help">${h.gpus?.status === "detected" ? tr("未检测到硬件显卡，将使用 CPU。", "No hardware GPU detected. CPU remains available.", "未偵測到硬體顯示卡，將使用 CPU。") : tr("暂时无法读取显卡信息，将继续使用 CPU。可点击重新检测。", "GPU inventory is unavailable. CPU remains available; try Detect again.", "暫時無法讀取顯示卡資訊，將繼續使用 CPU；可重新偵測。")}</p>`}</div>
  ${cudaOld ? `<p class="inline-error">${tr("当前 NVIDIA 驱动仅支持 CUDA", "The installed NVIDIA driver supports CUDA", "目前 NVIDIA 驅動程式僅支援 CUDA")} ${Math.floor(cudaVersion! / 1000)}.${Math.floor((cudaVersion! % 1000) / 10)}。${tr("此 CUDA 组件需要支持 CUDA 12 的驱动。请更新显卡驱动，或尝试 Vulkan；无需安装 CUDA 开发工具包。", "This CUDA component needs a CUDA 12 capable driver. Update the GPU driver or test Vulkan. No CUDA development toolkit is required.", "此 CUDA 元件需要支援 CUDA 12 的驅動程式。請更新顯示卡驅動程式或測試 Vulkan，不需安裝 CUDA 開發工具包。")}</p>` : ""}
  ${catalog.state === "not_published" ? `<p class="setting-help">${tr("GPU 组件尚未发布。显卡检测已完成；签名组件发布后可在这里检查并选择下载，当前继续使用 CPU。", "GPU components have not been published yet. Hardware detection is complete. Check here to choose a signed download when available; CPU remains active.", "GPU 元件尚未發布。硬體已偵測完成；簽章元件發布後可在此檢查及選擇下載，目前繼續使用 CPU。")}</p>` : ""}
  ${
    catalog.state !== "ready"
      ? `<p class="setting-help">${tr("检查组件目录只获取签名清单，不上传硬件信息或会议内容。", "Checking fetches only a signed catalog. Hardware information and meeting content are not uploaded.", "檢查只取得簽章清單，不上傳硬體資訊及會議內容。")}</p>`
      : catalog.components
          .map(({ component: c, version, present, installed, download: d }) => {
            const busy = d && ["downloading", "verifying"].includes(d.state);
            const unavailable =
              !devices.length || (c.backend === "cuda" && cudaBlocked);
            const incomplete = present && !installed && !busy;
            const remove =
              (present || installed) && !busy
                ? `<button class="subtle" data-component-action="remove" data-id="${esc(c.id)}" data-version="${esc(version)}" ${testing ? "disabled" : ""}>${glyph("trash")}${tr("移除", "Remove", "移除")}</button>`
                : "";
            const action = busy
              ? `<button class="outline" data-component-action="cancel" data-id="${esc(c.id)}">${tr("取消", "Cancel")}</button>`
              : incomplete
                ? ""
                : !installed
                  ? `<button class="outline" data-component-action="download" data-id="${esc(c.id)}" ${unavailable ? "disabled" : ""}>${glyph("download")}${tr("下载组件", "Download", "下載元件")}</button>`
                  : testing
                    ? matchesVerification(c.id, version)
                      ? `<button class="outline" data-component-action="cancel-test">${tr("取消测试", "Cancel test", "取消測試")}</button>`
                      : ""
                    : verification?.state === "active" &&
                        matchesVerification(c.id, version)
                      ? `<button class="outline" data-component-action="disable">${tr("使用 CPU", "Use CPU", "使用 CPU")}</button>`
                      : `<button class="outline" data-component-action="test" data-id="${esc(c.id)}" ${unavailable ? "disabled" : ""}>${tr("测试并启用", "Test & enable", "測試並啟用")}</button>`;
            const incompleteHint = incomplete
              ? `<p class="inline-error">${tr("安装不完整，请移除后重新下载。", "Installation is incomplete. Remove it, then download again.", "安裝不完整，請移除後重新下載。")}</p>`
              : "";
            return `<article class="model-card"><div class="model-icon">${glyph("models")}</div><div class="model-info"><div class="model-title"><h3>${esc(c.name)}</h3>${recommended(c.backend) ? `<span class="badge selected">${tr("推荐", "Recommended", "推薦")}</span>` : ""}<span class="badge">${installed ? statusLabel(c.id, version) : esc(c.backend.toUpperCase())}</span></div><p>${bytes(c.archive.bytes)} · ${esc(c.license)}</p>${busy ? `<progress max="${d.total}" value="${d.downloaded}"></progress><small>${bytes(d.downloaded)} / ${bytes(d.total)}</small>` : ""}${incompleteHint}${d?.error ? `<p class="inline-error">${tr("组件下载或校验失败，可以重试。", "Component download or verification failed. You can retry.", "元件下載或驗證失敗，可重試。")}</p>` : ""}</div><div class="model-actions">${action}${remove}</div></article>`;
          })
          .join("")
  }${failure}${comparison}`;
}
