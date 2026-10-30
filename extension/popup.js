const $ = (id) => document.getElementById(id);

function setStatus(kind, text) {
  $("status").className = `status ${kind}`;
  $("status-text").textContent = text;
}

async function checkApp() {
  const res = await chrome.runtime.sendMessage({ type: "ping" });
  if (res?.ok && res.reply?.running) {
    setStatus("ok", `Connected to the app (version ${res.reply.version})`);
  } else if (res?.ok) {
    setStatus("idle", "App installed; it starts when needed");
  } else {
    setStatus("missing", "App not found. Install it and open it once.");
    $("open").disabled = true;
  }
}

function fmtSize(n) {
  if (!n) return "";
  const units = ["B", "KB", "MB", "GB"];
  let i = 0;
  while (n >= 1024 && i < units.length - 1) { n /= 1024; i++; }
  return `${n.toFixed(i < 2 ? 0 : 1)} ${units[i]}`;
}

function showError(message) {
  $("error").textContent = message;
  $("error").hidden = false;
}

async function send(item, button) {
  button.disabled = true;
  const res = await chrome.runtime.sendMessage({ type: "download", item });
  if (res?.ok) window.close();
  else {
    showError(res?.error || "Couldn't reach the app");
    button.disabled = false;
  }
}

async function showMedia() {
  const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
  if (!tab) return;

  // Video sites: the page itself goes to yt-dlp.
  if (isMediaSite(tab.url)) {
    $("page-video").hidden = false;
    $("page-video").querySelector("b").textContent = "On this page";
    $("page-title").textContent = tab.title || tab.url;
    $("page-download").onclick = (e) =>
      send({ url: tab.url, media: true, title: tab.title || "", referrer: tab.url }, e.currentTarget);
    // Playlists and channels can be downloaded whole.
    const u = new URL(tab.url);
    const isList = u.searchParams.has("list") || /^\/(playlist|@|channel\/|c\/|user\/)/.test(u.pathname);
    $("page-playlist").hidden = !isList;
    $("page-playlist").onclick = (e) =>
      send({ url: tab.url, media: true, playlist: true, title: tab.title || "", referrer: tab.url }, e.currentTarget);
  }

  const { list = [] } = (await chrome.runtime.sendMessage({ type: "media-list", tabId: tab.id })) || {};
  if (list.length === 0) return;
  $("media").hidden = false;
  const ul = $("media-list");
  for (const m of [...list].reverse()) {
    const li = document.createElement("li");
    li.innerHTML = `<span class="kind"></span><span class="item-text"><span class="item-name"></span><span class="item-size"></span></span><button class="icon-btn" title="Download">⬇</button>`;
    li.querySelector(".kind").textContent = m.kind;
    li.querySelector(".kind").classList.add(m.kind);
    li.querySelector(".item-name").textContent = m.name;
    li.querySelector(".item-name").title = m.url;
    li.querySelector(".item-size").textContent =
      m.kind === "HLS" || m.kind === "DASH" ? "Stream (joined into one file)" : fmtSize(m.size);
    const stream = m.kind === "HLS" || m.kind === "DASH";
    li.querySelector("button").onclick = (e) =>
      send({ url: m.url, media: stream, title: m.title, referrer: tab.url }, e.currentTarget);
    ul.appendChild(li);
  }
}

(async () => {
  const opts = await chrome.storage.local.get({ catchDownloads: true, showVideoButton: true });
  $("catch").checked = opts.catchDownloads;
  $("catch").addEventListener("change", (e) => chrome.storage.local.set({ catchDownloads: e.target.checked }));
  $("video-button").checked = opts.showVideoButton;
  $("video-button").addEventListener("change", (e) => chrome.storage.local.set({ showVideoButton: e.target.checked }));
  showMedia();

  const { lastError } = (await chrome.storage.session?.get({ lastError: "" })) || {};
  if (lastError) showError(`Last problem: ${lastError}`);
  chrome.runtime.sendMessage({ type: "clear-error" });

  $("open").addEventListener("click", async () => {
    $("open").disabled = true;
    const res = await chrome.runtime.sendMessage({ type: "show" });
    if (res?.ok) window.close();
    else {
      $("error").textContent = res?.error || "Couldn't open the app";
      $("error").hidden = false;
      $("open").disabled = false;
    }
  });

  checkApp();
})();
