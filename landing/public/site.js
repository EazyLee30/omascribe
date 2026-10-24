import {
  validateProfile,
  validateSubmission,
  deepLink,
  MAX_PROFILE_BYTES,
} from "../../community/profile-schema.mjs";
const lang = document.documentElement.dataset.language || "zh";
const t = (zh, tw, en) => ({ zh, tw, en })[lang];
document.querySelector("#language")?.addEventListener("change", (event) => {
  location.href = event.target.value;
});
const dialog = document.querySelector("#profile-dialog");
if (dialog) {
  let catalog, profiles;
  // Filtering and navigation do not depend on catalog downloads. Attach their
  // listeners immediately so a fast interaction cannot be lost on a cold load.
  const catalogReady = (async () => {
    try {
      const responses = await Promise.all([
        fetch("/data/catalog.json"),
        fetch("/data/profiles.json"),
      ]);
      if (responses.some((r) => !r.ok))
        throw Error("Unable to load configuration catalog");
      [catalog, profiles] = await Promise.all(responses.map((r) => r.json()));
    } catch (error) {
      document.querySelector("#share-status").textContent = t(
        "目录加载失败，请刷新页面后重试。",
        "目錄載入失敗，請重新整理後重試。",
        "The catalog could not be loaded. Refresh to try again.",
      );
    }
  })();
  document.querySelectorAll(".preview-profile").forEach((button) =>
    button.addEventListener("click", async () => {
      button.loading = true;
      await catalogReady;
      button.loading = false;
      const entry = profiles?.find((p) => p.id === button.dataset.id);
      if (!entry || !catalog) return;
      try {
        const p = validateProfile(entry.profile, catalog);
        document.querySelector("#preview-title").textContent =
          entry.title[lang];
        document.querySelector("#preview-json").textContent = JSON.stringify(
          p,
          null,
          2,
        );
        document.querySelector("#preview-status").textContent = "";
        document.querySelector("#download-profile").href =
          `/data/profiles/${entry.id}.json`;
        const open = document.querySelector("#open-app");
        try {
          open.href = deepLink(p);
          open.removeAttribute("aria-disabled");
        } catch {
          open.removeAttribute("href");
          open.setAttribute("aria-disabled", "true");
          document.querySelector("#preview-status").textContent = t(
            "配置较长，请下载 JSON 后导入应用。",
            "設定較長，請下載 JSON 後匯入應用。",
            "This configuration is too long for an app link. Download and import its JSON file.",
          );
        }
        dialog.open = true;
      } catch (error) {
        document.querySelector("#share-status").textContent = String(
          error.message,
        );
      }
    }),
  );
  const filters = ["platform", "memory", "origin"].map((key) =>
    document.querySelector(`#${key}-filter`),
  );
  const filter = () => {
    const [platform, memory, origin] = filters.map((x) => x.value);
    const query = (document.querySelector("#profile-search").value || "")
      .trim()
      .toLocaleLowerCase();
    let visible = 0;
    document.querySelectorAll(".profile-card").forEach((card) => {
      card.hidden =
        !!(query && !card.textContent.toLocaleLowerCase().includes(query)) ||
        !(
          platform === "all" ||
          card.dataset.platform === platform ||
          card.dataset.platform === "cross-platform"
        ) ||
        !(memory === "all" || Number(card.dataset.memory) <= Number(memory)) ||
        !(origin === "all" || card.dataset.origin === origin);
      if (!card.hidden) visible++;
    });
    document.querySelector("#no-results").hidden = visible > 0;
  };
  filters.forEach((field) => field.addEventListener("change", filter));
  document.querySelector("#profile-search").addEventListener("input", filter);
  document
    .querySelector("#profile-search")
    .addEventListener("wa-clear", filter);
  Promise.all(filters.map((field) => field.updateComplete)).then(filter);
  let localProfile = null,
    readId = 0;
  const status = document.querySelector("#share-status"),
    prepare = document.querySelector("#prepare-submission");
  document
    .querySelector("#profile-file")
    .addEventListener("change", async (event) => {
      const id = ++readId;
      localProfile = null;
      prepare.disabled = true;
      document.querySelector("#file-preview").hidden = true;
      document.querySelector("#submission-next").hidden = true;
      const file = event.target.files?.[0];
      if (!file) return;
      try {
        if (file.size > MAX_PROFILE_BYTES)
          throw Error("Configuration exceeds 64 KiB");
        await catalogReady;
        if (readId !== id) return;
        if (!catalog) throw Error("Configuration catalog is unavailable");
        const raw = await file.text();
        if (readId !== id) return;
        const p = validateProfile(raw, catalog);
        localProfile = p;
        document.querySelector("#file-name").textContent = p.name;
        document.querySelector("#file-json").textContent = JSON.stringify(
          p,
          null,
          2,
        );
        document.querySelector("#file-preview").hidden = false;
        prepare.disabled = false;
        status.textContent = t(
          "校验通过。文件尚未上传；请核对下面的公开信息。",
          "驗證通過。檔案尚未上傳，請核對公開資訊。",
          "Validated locally. Nothing has been uploaded; review the public fields.",
        );
        status.classList.remove("error");
      } catch (error) {
        if (readId !== id) return;
        status.textContent =
          t(
            "无法使用该配置：",
            "無法使用此設定：",
            "Configuration rejected: ",
          ) + error.message;
        status.classList.add("error");
      }
    });
  document.querySelector("#share-form").addEventListener("submit", (event) => {
    event.preventDefault();
    if (!localProfile || !catalog) return;
    try {
      const id = `community-${crypto.randomUUID().slice(0, 8)}`;
      const description = (
        document.querySelector("#share-description").value || ""
      ).trim();
      const title = localProfile.name;
      const submission = validateSubmission(
        {
          schema: 1,
          id,
          origin: "community",
          author: (document.querySelector("#share-author").value || "").trim(),
          title: { zh: title, tw: title, en: title },
          description: { zh: description, tw: description, en: description },
          platform: document.querySelector("#share-platform").value,
          memory_gib: Number(document.querySelector("#share-memory").value),
          profile: localProfile,
          benchmark_ids: [],
        },
        catalog,
      );
      const blob = new Blob([JSON.stringify(submission, null, 2) + "\n"], {
        type: "application/json",
      });
      const href = URL.createObjectURL(blob),
        a = document.createElement("a");
      a.href = href;
      a.download = `${id}.json`;
      a.click();
      setTimeout(() => URL.revokeObjectURL(href), 1000);
      status.textContent = t(
        "投稿文件已生成，尚未上传或发布。",
        "投稿檔案已產生，尚未上傳或發布。",
        "Submission file generated. It has not been uploaded or published.",
      );
      status.classList.remove("error");
      document.querySelector("#submission-next").hidden = false;
    } catch (error) {
      status.textContent = error.message;
      status.classList.add("error");
    }
  });
}
