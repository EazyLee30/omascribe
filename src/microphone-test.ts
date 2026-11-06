import { invoke } from "@tauri-apps/api/core";
import { tr } from "./workspace-ui";

interface InputLevel {
  active: boolean;
  starting: boolean;
  device: string;
  rms: number;
  peak: number;
  waveform: number[];
  elapsed_ms: number;
  error: string | null;
}

export function mountMicrophoneTest(
  root: HTMLElement,
  device: HTMLSelectElement,
) {
  let closed = false,
    busy = false,
    polling = false;
  let epoch = 0;
  let level: InputLevel | null = null;
  const selected = () => device.selectedOptions[0];
  const supported = () =>
    !["system", "mixed"].includes(selected()?.dataset.kind || "") &&
    !!device.value;
  root.innerHTML = `<section class="microphone-test"><div class="microphone-test-heading"><div><h2>${tr("测试麦克风", "Test microphone", "測試麥克風")}</h2><p data-microphone-name></p></div><button class="outline" data-microphone-test></button></div><svg class="microphone-waveform" viewBox="0 0 320 64" preserveAspectRatio="none" role="img" aria-label="${tr("麦克风输入电平波形", "Microphone input level waveform", "麥克風輸入電平波形")}"><path></path></svg><div class="microphone-test-level"><span data-microphone-status role="status"></span><span data-microphone-percent></span></div><p class="setting-help">${tr("只检测所选输入，不保存录音，也不转写或上传。30 秒后自动停止。", "Tests the selected input without saving, transcribing or uploading audio. Stops automatically after 30 seconds.", "只檢測所選輸入，不儲存錄音、轉寫或上傳，30 秒後自動停止。")}</p><details hidden><summary>${tr("详细信息", "Details", "詳細資訊")}</summary><p data-microphone-error></p></details></section>`;
  const button = root.querySelector<HTMLButtonElement>(
    "[data-microphone-test]",
  )!;
  const statusText = root.querySelector<HTMLElement>(
    "[data-microphone-status]",
  )!;
  const draw = () => {
    if (closed) return;
    const active = level?.active || false;
    const peaks = level?.waveform || [];
    const points = peaks.map((value, index) => [
      (index * 320) / Math.max(1, peaks.length - 1),
      Math.sqrt(Math.max(0, Math.min(1, value))) * 28,
    ]);
    const path = points.length
      ? `M ${points.map(([x, y]) => `${x.toFixed(1)},${(32 - y).toFixed(1)}`).join(" L ")} L ${points
          .slice()
          .reverse()
          .map(([x, y]) => `${x.toFixed(1)},${(32 + y).toFixed(1)}`)
          .join(" L ")} Z`
      : "M 0,32 L 320,32";
    const status = level?.error
      ? tr(
          "无法读取输入，请检查设备连接和麦克风权限。",
          "Could not read input. Check the device connection and microphone permission.",
          "無法讀取輸入，請檢查裝置連接和麥克風權限。",
        )
      : level?.starting
        ? tr("正在打开麦克风…", "Opening microphone…", "正在開啟麥克風…")
        : active
          ? level && level.peak >= 0.01
            ? tr("检测到输入", "Input detected", "偵測到輸入")
            : tr(
                "输入很低，请对着麦克风说话",
                "Input is low. Speak into the microphone.",
                "輸入很低，請對著麥克風說話",
              )
          : tr("检测未运行", "Test is not running", "檢測未執行");
    root.querySelector("path")!.setAttribute("d", path);
    root.querySelector("[data-microphone-name]")!.textContent =
      active && level?.device ? level.device : selected()?.textContent || "";
    button.disabled = busy || !supported();
    button.setAttribute("aria-pressed", String(active));
    button.textContent = active
      ? tr("停止检测", "Stop test", "停止檢測")
      : tr("开始检测", "Start test", "開始檢測");
    if (statusText.textContent !== status) statusText.textContent = status;
    root.querySelector("[data-microphone-percent]")!.textContent =
      `${Math.round((level?.peak || 0) * 100)}%`;
    root.querySelector<HTMLDetailsElement>("details")!.hidden = !level?.error;
    root.querySelector("[data-microphone-error]")!.textContent =
      level?.error || "";
  };
  const stop = async () => {
    level = await invoke<InputLevel>("stop_microphone_test");
    draw();
  };
  const click = async (event: Event) => {
    if (
      !(event.target as HTMLElement).closest("[data-microphone-test]") ||
      busy
    )
      return;
    busy = true;
    epoch++;
    draw();
    try {
      level = level?.active
        ? await invoke<InputLevel>("stop_microphone_test")
        : await invoke<InputLevel>("start_microphone_test", {
            device: device.value,
          });
      if (closed) await invoke("stop_microphone_test");
    } catch (error) {
      level = {
        active: false,
        starting: false,
        device: device.value,
        rms: 0,
        peak: 0,
        waveform: [],
        elapsed_ms: 0,
        error: String(error),
      };
    } finally {
      busy = false;
      draw();
    }
  };
  const changed = async () => {
    busy = true;
    epoch++;
    draw();
    try {
      await stop();
      level = null;
    } catch (error) {
      if (level) level.error = String(error);
    } finally {
      busy = false;
      draw();
    }
  };
  root.addEventListener("click", click);
  device.addEventListener("change", changed);
  const timer = setInterval(async () => {
    if (closed || !level?.active || polling || busy) return;
    polling = true;
    const requested = epoch;
    try {
      const next = await invoke<InputLevel>("microphone_test_status");
      if (requested === epoch) {
        level = next;
        draw();
      }
    } catch (error) {
      if (requested === epoch && level) {
        level.active = false;
        level.error = String(error);
        draw();
      }
    } finally {
      polling = false;
    }
  }, 150);
  draw();
  return {
    async close() {
      closed = true;
      epoch++;
      clearInterval(timer);
      root.removeEventListener("click", click);
      device.removeEventListener("change", changed);
      await invoke("stop_microphone_test");
    },
  };
}
