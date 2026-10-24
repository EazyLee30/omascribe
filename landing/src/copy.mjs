export const LANGS = ['zh','tw','en'];
export const t = (lang, zh, tw, en) => ({zh,tw,en})[lang];
export const pathFor = (lang, page = '') => `${lang === 'zh' ? '' : lang === 'tw' ? '/zh-TW' : '/en'}/${page ? `${page}/` : ''}`;
export const copy = {
  nav: {zh:['首页','下载','模型与性能','配置广场','隐私说明'],tw:['首頁','下載','模型與效能','設定廣場','隱私說明'],en:['Overview','Download','Models & performance','Configurations','Privacy']},
  pages: ['', 'download', 'models', 'community', 'privacy'],
  titles: {zh:['本地录音、转写和会议纪要','下载 OmaScribe','模型与性能','配置广场','哪些联网，哪些留在本机'],tw:['本機錄音、轉寫和會議紀要','下載 OmaScribe','模型與效能','設定廣場','哪些連網，哪些留在本機'],en:['Local recording, transcription and meeting notes','Download OmaScribe','Models & performance','Configuration gallery','What connects, and what stays local']},
};
