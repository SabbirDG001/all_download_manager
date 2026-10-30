// Catches browser downloads and sends them to the All Download Manager app.

const HOST = "com.alldownloadmanager.host";
const DEFAULTS = { catchDownloads: true };

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

async function sendDownload({ url, referrer = "", filename = "" }) {
  await callApp({
    action: "add",
    url,
    referrer,
    filename,
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

// Messages from the popup.
chrome.runtime.onMessage.addListener((msg, _sender, respond) => {
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
