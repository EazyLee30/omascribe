import { invoke } from "@tauri-apps/api/core";
import type { Snapshot, Segment, Meeting } from "./types";
import { tr, esc, glyph } from "./workspace-ui";
export function speakerName(segment: Segment, meeting: Meeting) {
  const name = segment.speaker
    ? meeting.speaker_names?.[segment.speaker]
    : null;
  return name?.match(/^Speaker (\d+)$/)
    ? name.replace("Speaker", tr("说话人", "Speaker", "說話人"))
    : name || tr("未分配", "Unassigned", "未分配");
}
export async function speakerDialog(
  dialog: HTMLDialogElement,
  meeting: string,
  segmentId: string | undefined,
  refresh: () => Promise<void>,
  toast: (s: string, error?: boolean) => void,
) {
  let snapshot = await invoke<Snapshot>("snapshot", { id: meeting });
  function body() {
    const m = snapshot.meeting,
      r = snapshot.runtime,
      segment = snapshot.segments.find((s) => s.id === segmentId),
      running = r.diarization_busy;
    dialog.innerHTML = `<div class="modal-heading"><h2>${glyph("speakers")}${tr("说话人", "Speakers", "說話人")}</h2><button class="subtle" data-speaker-action="close" aria-label="${tr("关闭", "Close", "關閉")}">×</button></div><p class="setting-help">${tr("先转写，再分析声音。可以修改姓名和每条转写的归属；自动识别结果请结合音频核对。", "Transcribe first, then analyze voices. Edit names and passage assignments, and check automatic labels against the audio.", "先轉寫再分析聲音，可修改姓名及歸屬，請配合音訊核對。")}</p><label class="check-setting"><input id="meeting-diarization" type="checkbox" ${m.diarization ? "checked" : ""} ${running ? "disabled" : ""}>${tr("本场会议区分说话人", "Separate speakers in this meeting", "本場會議區分說話人")}</label><label class="check-setting"><input id="meeting-auto-notes" type="checkbox" ${m.auto_notes ? "checked" : ""}>${tr("自动整理本场纪要", "Write minutes automatically for this meeting", "自動整理本場紀要")}</label><div class="speaker-status" role="status">${running ? tr("正在分析声音…", "Analyzing voices…", "正在分析聲音…") : r.diarization_meeting_id === meeting && r.diarization_status ? (r.diarization_status === "done" ? tr("分析完成", "Analysis complete") : esc(r.diarization_status)) : ""}</div><div class="button-row"><button class="outline" data-speaker-action="${running ? "cancel" : "analyze"}" ${!snapshot.segments.length ? "disabled" : ""}>${running ? tr("取消分析", "Cancel analysis") : tr("分析 / 重新分析", "Analyze / analyze again")}</button><button class="text-button" data-speaker-action="models">${tr("模型与默认设置", "Models & defaults", "模型及預設設定")}</button></div><p class="setting-help">${tr("重新分析会重新编号自动分组，手动分配的片段会保留。最长支持 2 小时。", "Reanalysis numbers automatic clusters again; manual assignments are preserved. Up to 2 hours per recording.", "重新分析會重新編號自動分組，保留手動歸屬，最長 2 小時。")}</p>${
      segment
        ? `<div class="speaker-assignment"><p>${esc(segment.text)}</p><label>${tr("此片段的说话人", "Speaker for this passage", "此片段的說話人")}<select id="passage-speaker"><option value="">${tr("未分配", "Unassigned")}</option>${Object.entries(
            m.speaker_names || {},
          )
            .map(
              ([id, name]) =>
                `<option value="${esc(id)}" ${id === segment.speaker ? "selected" : ""}>${esc(name)}</option>`,
            )
            .join(
              "",
            )}</select></label><button class="outline" data-speaker-action="assign">${tr("保存归属", "Save assignment", "儲存歸屬")}</button></div>`
        : ""
    }<h3 class="settings-subtitle">${tr("姓名", "Names")}</h3><div class="speaker-list">${Object.entries(
      m.speaker_names || {},
    )
      .map(
        ([id, name]) =>
          `<div class="speaker-row"><input value="${esc(name)}" maxlength="60" data-speaker-name="${esc(id)}" aria-label="${tr("说话人姓名", "Speaker name", "說話人姓名")}"><button class="outline" data-speaker-rename="${esc(id)}">${tr("保存", "Save", "儲存")}</button></div>`,
      )
      .join(
        "",
      )}</div><div class="speaker-row"><input id="new-speaker-name" maxlength="60" placeholder="${tr("添加一个姓名", "Add a name", "新增姓名")}" aria-label="${tr("新说话人姓名", "New speaker name", "新說話人姓名")}"><button class="outline" data-speaker-action="add">${glyph("plus")}${tr("添加", "Add", "新增")}</button></div>`;
  }
  body();
  if (!dialog.open) dialog.showModal();
  const reload = async () => {
    snapshot = await invoke<Snapshot>("snapshot", { id: meeting });
    body();
    await refresh();
  };
  const click = async (event: Event) => {
    const b = (event.target as HTMLElement).closest<HTMLElement>("button");
    if (!b) return;
    try {
      if (b.dataset.speakerRename) {
        const id = b.dataset.speakerRename;
        const name = dialog.querySelector<HTMLInputElement>(
          `[data-speaker-name="${CSS.escape(id)}"]`,
        )!.value;
        await invoke("rename_speaker", { meeting, id, name });
        await reload();
        return;
      }
      switch (b.dataset.speakerAction) {
        case "close":
          dialog.close();
          break;
        case "models":
          dialog.close();
          document.dispatchEvent(
            new CustomEvent("open-settings", { detail: "models" }),
          );
          break;
        case "analyze":
          await invoke("diarize", { id: meeting });
          await reload();
          break;
        case "cancel":
          await invoke("cancel_diarization");
          break;
        case "add": {
          const name =
            dialog.querySelector<HTMLInputElement>("#new-speaker-name")!.value;
          await invoke("rename_speaker", { meeting, id: null, name });
          await reload();
          break;
        }
        case "assign": {
          const segment = snapshot.segments.find((s) => s.id === segmentId)!;
          await invoke("assign_speaker", {
            meeting,
            segment: segment.id,
            speaker:
              dialog.querySelector<HTMLSelectElement>("#passage-speaker")!
                .value || null,
            revision: segment.revision,
          });
          await reload();
          break;
        }
      }
    } catch (e) {
      toast(String(e), true);
    }
  };
  const change = async (event: Event) => {
    const input = event.target as HTMLInputElement;
    try {
      if (input.id === "meeting-diarization")
        await invoke("set_meeting_diarization", {
          id: meeting,
          enabled: input.checked,
        });
      if (input.id === "meeting-auto-notes")
        await invoke("update_meeting", {
          id: meeting,
          title: snapshot.meeting.title,
          autoNotes: input.checked,
        });
      await refresh();
    } catch (e) {
      toast(String(e), true);
      await reload();
    }
  };
  dialog.addEventListener("click", click);
  dialog.addEventListener("change", change);
  const timer = setInterval(() => {
    if (snapshot.runtime.diarization_busy) void reloadStatus();
  }, 1000);
  async function reloadStatus() {
    try {
      const next = await invoke<Snapshot>("snapshot", { id: meeting });
      if (!dialog.open) return;
      const ended =
        snapshot.runtime.diarization_busy && !next.runtime.diarization_busy;
      snapshot = next;
      if (ended) {
        body();
        await refresh();
      }
    } catch (e) {
      toast(String(e), true);
    }
  }
  dialog.addEventListener(
    "close",
    () => {
      clearInterval(timer);
      dialog.removeEventListener("click", click);
      dialog.removeEventListener("change", change);
    },
    { once: true },
  );
}
