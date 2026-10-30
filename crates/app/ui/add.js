"use strict";

// "Download File Info": the small window that pops up for a link from the
// browser, like IDM's. The main window stays where it is (often in the tray).

const { invoke } = window.__TAURI__.core;
const $ = (sel) => document.querySelector(sel);

function fmtBytes(n) {
  if (n == null) return "";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let i = 0;
  while (n >= 1024 && i < units.length - 1) { n /= 1024; i++; }
  return `${n.toFixed(i === 0 ? 0 : 2)} ${units[i]}`;
}

const KIND_OF_EXT = {
  video: ["mp4", "mkv", "webm", "avi", "mov", "flv", "wmv", "m4v", "ts"],
  audio: ["mp3", "m4a", "flac", "wav", "aac", "ogg", "opus", "wma"],
  archive: ["zip", "rar", "7z", "tar", "gz", "bz2", "xz", "iso", "zst"],
  program: ["exe", "msi", "msix", "apk", "dmg", "deb", "appimage"],
  document: ["pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "txt", "epub", "csv"],
  image: ["jpg", "jpeg", "png", "gif", "webp", "svg", "bmp", "heic"],
};

function showExt(name, fallback = "file") {
  const m = /\.([a-z0-9]{1,5})$/i.exec(name || "");
  const ext = m ? m[1].toLowerCase() : fallback;
  $("#ext").textContent = ext.slice(0, 4);
  $("#ext").dataset.kind = Object.entries(KIND_OF_EXT).find(([, l]) => l.includes(ext))?.[0] || "other";
}

let item = null;
let settings = null;
let formats = []; // quality choices, by <option> value

const close = () => invoke("close_this_window");

function setBusy(text) {
  $("#status").textContent = text || "";
}

async function loadFile() {
  showExt(item.filename);
  if (item.url.startsWith("ftp://")) {
    $("#name").value = item.filename || decodeURIComponent(item.url.split("/").pop());
    showExt($("#name").value);
    return;
  }
  setBusy("Getting file information…");
  try {
    const info = await invoke("get_link_info", { url: item.url, headers: item.headers });
    if (!$("#name").value) $("#name").value = info.filename;
    showExt($("#name").value);
    $("#size").textContent = info.size != null ? fmtBytes(info.size) : "Size unknown";
    setBusy(info.resumable ? "" : "This server can't resume: a paused download starts over.");
  } catch (err) {
    setBusy(`Couldn't reach the file (${err}). You can still try to download it.`);
  }
}

async function loadFormats() {
  $("#name-label").textContent = "Title";
  $("#name").value = item.title || "";
  $("#name").readOnly = true;
  $("#quality-label").hidden = false;
  $("#quality").hidden = false;
  showExt("", "mp4");
  const select = $("#quality");
  const fallback = [
    ["v:", "Best video"], ["v:1080", "Up to 1080p"], ["v:720", "Up to 720p"], ["v:480", "Up to 480p"],
    ["a:mp3:320", "Audio · MP3 320 kbps"], ["a:mp3:128", "Audio · MP3 128 kbps"],
  ];
  select.innerHTML = "";
  for (const [v, label] of fallback) select.add(new Option(label, v));
  if (item.format) {
    // Picked on the page already.
    select.innerHTML = "";
    select.add(new Option(describe(item.format), "picked"));
    formats = [{ value: "picked", format: item.format }];
    return;
  }
  setBusy("Checking which qualities are available…");
  try {
    const info = await invoke("get_media_formats", { url: item.url, headers: item.headers });
    if (info.title) $("#name").value = info.title;
    select.innerHTML = "";
    formats = [];
    const group = (label, list) => {
      if (!list.length) return;
      const og = document.createElement("optgroup");
      og.label = label;
      for (const opt of list) {
        const value = String(formats.length);
        formats.push({ value, format: opt.format, size: opt.size });
        const text = [opt.label, opt.detail, opt.size ? `~${fmtBytes(opt.size)}` : ""].filter(Boolean).join("  ·  ");
        og.appendChild(new Option(text, value));
      }
      select.appendChild(og);
    };
    group("Video", info.video);
    group("Audio only", info.audio);
    pickDefault();
    setBusy(info.needsFfmpeg ? "Install ffmpeg in Settings for HD video and MP3." : "");
  } catch (err) {
    setBusy(String(err));
  }
  updateSize();
}

function describe(f) {
  if (f.type === "video") return f.height ? `${f.height}p` : "Best video";
  return f.codec === "mp3" ? `MP3 ${f.kbps || ""} kbps` : f.codec.toUpperCase();
}

function pickDefault() {
  const q = settings.video_quality;
  const limit = { "1080p": 1080, "720p": 720, "480p": 480 }[q] || Infinity;
  const pick = q === "audio"
    ? formats.find((f) => f.format.type === "audio")
    : formats.find((f) => f.format.type === "video" && (f.format.height ?? 0) <= limit);
  if (pick) $("#quality").value = pick.value;
}

function updateSize() {
  const chosen = formats.find((f) => f.value === $("#quality").value);
  $("#size").textContent = chosen?.size ? `~${fmtBytes(chosen.size)}` : "";
  showExt("", chosen?.format.type === "audio" ? "mp3" : "mp4");
}

function chosenFormat() {
  const v = $("#quality").value;
  const found = formats.find((f) => f.value === v);
  if (found) return found.format;
  const [kind, a, b] = v.split(":");
  if (kind === "v") return { type: "video", height: a ? Number(a) : null };
  return { type: "audio", codec: a, kbps: b ? Number(b) : null };
}

async function submit(later) {
  const dir = $("#dir").value.trim();
  const request = {
    url: $("#url").value.trim(),
    // The default folder is left unset so "sort into folders" still applies.
    dir: dir && dir !== settings.download_dir ? dir : null,
    filename: item.media ? null : $("#name").value.trim() || null,
    media: item.media,
    format: item.media ? chosenFormat() : null,
    title: item.media ? $("#name").value : item.title,
    headers: item.headers,
    paused: later && !settings.schedule.enabled,
    scheduled: later && settings.schedule.enabled,
  };
  $("#start").disabled = $("#later").disabled = true;
  try {
    await invoke("add_and_watch", { request });
    close();
  } catch (err) {
    setBusy(String(err));
    $("#start").disabled = $("#later").disabled = false;
  }
}

$("#form").addEventListener("submit", (ev) => { ev.preventDefault(); submit(false); });
$("#later").onclick = () => submit(true);
$("#cancel").onclick = close;
$("#quality").addEventListener("change", updateSize);
$("#browse").onclick = async () => {
  const folder = await invoke("pick_folder", { start: $("#dir").value || null });
  if (folder) $("#dir").value = folder;
};
document.addEventListener("keydown", (ev) => { if (ev.key === "Escape") close(); });

(async () => {
  const key = Number(new URLSearchParams(location.search).get("key"));
  [item, settings] = await Promise.all([
    invoke("take_browser_download", { key }),
    invoke("get_settings").then((i) => i.settings),
  ]);
  if (!item) { close(); return; }
  $("#url").value = item.url;
  $("#dir").value = settings.download_dir;
  $("#name").value = item.filename || "";
  $("#later").title = settings.schedule.enabled ? "Add it to the scheduler" : "Add it paused, to start later";
  $("#start").focus();
  if (item.media) loadFormats(); else loadFile();
})();
