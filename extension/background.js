// Catches browser downloads and sends them to the All Download Manager app,
// and finds video and audio playing in pages.

importScripts("sites.js");

const HOST = "com.alldownloadmanager.host";
const DEFAULTS = { catchDownloads: true, showVideoButton: true };

async function options() {
  return chrome.storage.local.get(DEFAULTS);
}

function isWebUrl(url) {
  return /^https?:\/\//i.test(url || "");
}

async function cookieHeader(url) {
  try {
    const cookies = await chrome.cookies.getAll({ url });
    return cookies.map((c) => `${c.name}=${c.value}`).join("; ");
  } catch {
    return "";
  }
}

/** Sends a message to the app (starting it if needed) and returns its answer. */
async function callApp(message) {
  const reply = await chrome.runtime.sendNativeMessage(HOST, message);
  if (!reply || !reply.ok) throw new Error(reply?.error || "No answer from the app");
  return reply;
}

async function sendDownload({ url, referrer = "", filename = "", media = false, title = "" }) {
  await callApp({
    action: "add",
    url,
    referrer,
    filename,
    media,
    title,
    cookies: await cookieHeader(url),
    userAgent: navigator.userAgent,
  });
  setError(null);
}

function setError(message) {
  chrome.action.setBadgeText({ text: message ? "!" : "" });
  chrome.action.setBadgeBackgroundColor({ color: "#dc2626" });
  chrome.action.setTitle({
    title: message ? `All Download Manager: ${message}` : "All Download Manager",
  });
  chrome.storage.session?.set({ lastError: message || "" });
}

function baseName(path) {
  return (path || "").split(/[\\/]/).pop();
}

// Downloads started by the browser (clicked links, "Save link as", etc.).
chrome.downloads.onCreated.addListener(async (item) => {
  const url = item.finalUrl || item.url;
  if (!isWebUrl(url) || item.incognito || item.state !== "in_progress") return;
  if (!(await options()).catchDownloads) return;

  // Hold the browser's download while asking the app to take it.
  await chrome.downloads.pause(item.id).catch(() => {});
  try {
    await sendDownload({ url, referrer: item.referrer, filename: baseName(item.filename) });
    await chrome.downloads.cancel(item.id).catch(() => {});
    await chrome.downloads.erase({ id: item.id }).catch(() => {});
  } catch (err) {
    // The app isn't available: let the browser finish the download itself.
    await chrome.downloads.resume(item.id).catch(() => {});
    setError(err.message);
  }
});

// Right-click menu on links and media.
chrome.runtime.onInstalled.addListener(() => {
  chrome.contextMenus.removeAll(() => {
    chrome.contextMenus.create({
      id: "adm-link",
      title: "Download link with All Download Manager",
      contexts: ["link"],
    });
    chrome.contextMenus.create({
      id: "adm-media",
      title: "Download with All Download Manager",
      contexts: ["image", "video", "audio"],
    });
  });
});

chrome.contextMenus.onClicked.addListener(async (info) => {
  const url = info.menuItemId === "adm-link" ? info.linkUrl : info.srcUrl;
  if (!isWebUrl(url)) {
    setError("Only http and https links can be downloaded");
    return;
  }
  try {
    await sendDownload({ url, referrer: info.pageUrl || "" });
  } catch (err) {
    setError(err.message);
  }
});

// ---------- Finding video and audio in pages ----------

const STREAM_TYPES = [
  "application/vnd.apple.mpegurl", "application/x-mpegurl", "audio/mpegurl", "audio/x-mpegurl",
  "application/dash+xml",
];
const MEDIA_FILE = /\.(mp4|webm|mkv|mov|m4v|flv|mp3|m4a|aac|ogg|oga|opus|wav|flac)$/i;
const AUDIO_FILE = /\.(mp3|m4a|aac|ogg|oga|opus|wav|flac)$/i;
// Pieces of a stream rather than whole files.
const SEGMENT = /\.(ts|m4s|m4f|m4v\d|cmfv|cmfa|vtt|key)$|\/(seg|segment|chunk|frag)[-_]?\d+/i;
const MIN_FILE_SIZE = 256 * 1024;
const MAX_ITEMS = 30;

function header(details, name) {
  const h = details.responseHeaders?.find((x) => x.name.toLowerCase() === name);
  return h ? h.value : "";
}

/** The full size of the file, also for responses to range requests. */
function fullSize(details) {
  const range = /\/(\d+)$/.exec(header(details, "content-range"));
  if (range) return Number(range[1]);
  const len = Number(header(details, "content-length"));
  return len > 0 ? len : null;
}

function classify(details) {
  if (details.statusCode >= 400 || !isWebUrl(details.url)) return null;
  const type = header(details, "content-type").split(";")[0].trim().toLowerCase();
  let path;
  try { path = new URL(details.url).pathname; } catch { return null; }
  if (STREAM_TYPES.includes(type) || /\.m3u8$/i.test(path)) return { kind: "HLS", type };
  if (type === "application/dash+xml" || /\.mpd$/i.test(path)) return { kind: "DASH", type };
  const looksMedia = /^(video|audio)\//.test(type) || MEDIA_FILE.test(path);
  if (!looksMedia || SEGMENT.test(path) || type === "video/mp2t") return null;
  const size = fullSize(details);
  if (size != null && size < MIN_FILE_SIZE) return null;
  const audio = type.startsWith("audio/") || AUDIO_FILE.test(path);
  return { kind: audio ? "Audio" : "Video", type, size };
}

const sessionKey = (tabId) => `media:${tabId}`;

async function mediaFor(tabId) {
  const key = sessionKey(tabId);
  return (await chrome.storage.session.get({ [key]: [] }))[key];
}

async function setMedia(tabId, list) {
  await chrome.storage.session.set({ [sessionKey(tabId)]: list });
  chrome.action.setBadgeBackgroundColor({ tabId, color: "#2563eb" });
  chrome.action.setBadgeText({ tabId, text: list.length ? String(list.length) : "" });
}

// Requests are handled one at a time so the list isn't overwritten.
let queue = Promise.resolve();

chrome.webRequest.onHeadersReceived.addListener(
  (details) => {
    if (details.tabId < 0) return;
    const found = classify(details);
    if (!found) return;
    queue = queue.then(async () => {
      // Video sites are handled by yt-dlp from the page address instead.
      const tab = await chrome.tabs.get(details.tabId).catch(() => null);
      if (!tab || isMediaSite(tab.url)) return;
      const list = await mediaFor(details.tabId);
      const id = details.url.split("#")[0];
      if (list.some((m) => m.url === id)) return;
      const name = decodeURIComponent(new URL(id).pathname.split("/").pop() || "media");
      list.push({ url: id, name, ...found, title: tab.title || "" });
      await setMedia(details.tabId, list.slice(-MAX_ITEMS));
    }).catch(() => {});
  },
  { urls: ["<all_urls>"], types: ["media", "xmlhttprequest", "other", "object"] },
  ["responseHeaders"],
);

// A new page starts with an empty list.
chrome.tabs.onUpdated.addListener((tabId, change) => {
  if (change.url) queue = queue.then(() => setMedia(tabId, [])).catch(() => {});
});
chrome.tabs.onRemoved.addListener((tabId) => {
  chrome.storage.session.remove(sessionKey(tabId));
});

/** The best thing to download for the video the user pointed at. */
async function pickVideo({ src, pageUrl }, tab) {
  const tabUrl = tab?.url || pageUrl;
  // Video sites (also embedded players, whose frame is on the video site).
  if (isMediaSite(pageUrl)) return { url: pageUrl, media: true };
  if (isMediaSite(tabUrl)) return { url: tabUrl, media: true };
  const found = tab ? await mediaFor(tab.id) : [];
  const stream = found.find((m) => m.kind === "HLS" || m.kind === "DASH");
  if (stream) return { url: stream.url, media: true };
  // A plain video file: the app's own parallel download is fastest.
  if (isWebUrl(src)) return { url: src, media: false };
  const files = found.filter((m) => m.kind === "Video").sort((a, b) => (b.size || 0) - (a.size || 0));
  if (files.length) return { url: files[0].url, media: false };
  // Let yt-dlp look at the page itself.
  return { url: pageUrl, media: true };
}

// Messages from the popup and the page button.
chrome.runtime.onMessage.addListener((msg, sender, respond) => {
  if (msg?.type === "grab-video") {
    (async () => {
      const tab = sender.tab;
      const choice = await pickVideo(msg, tab);
      await sendDownload({ ...choice, title: msg.title || tab?.title || "", referrer: tab?.url || msg.pageUrl });
    })()
      .then(() => respond({ ok: true }))
      .catch((err) => { setError(err.message); respond({ ok: false, error: err.message }); });
    return true;
  }
  if (msg?.type === "media-list") {
    mediaFor(msg.tabId).then((list) => respond({ list }));
    return true;
  }
  if (msg?.type === "download") {
    sendDownload(msg.item)
      .then(() => respond({ ok: true }))
      .catch((err) => respond({ ok: false, error: err.message }));
    return true;
  }
  if (msg?.type === "ping") {
    chrome.runtime
      .sendNativeMessage(HOST, { action: "ping" })
      .then((reply) => respond({ ok: true, reply }))
      .catch((err) => respond({ ok: false, error: err.message }));
    return true;
  }
  if (msg?.type === "show") {
    callApp({ action: "show" })
      .then(() => respond({ ok: true }))
      .catch((err) => respond({ ok: false, error: err.message }));
    return true;
  }
  if (msg?.type === "clear-error") {
    setError(null);
  }
  return false;
});
