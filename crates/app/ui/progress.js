"use strict";

// The small per-download progress window, like IDM's. It follows one
// download and turns into "Download complete" with Open buttons at the end.

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = (sel) => document.querySelector(sel);

const id = Number(new URLSearchParams(location.search).get("id"));
let entry = null;
let detailsOpen = false;
let lastTitle = "";

function fmtBytes(n) {
  if (n == null) return "—";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let i = 0;
  while (n >= 1024 && i < units.length - 1) { n /= 1024; i++; }
  return `${n.toFixed(i === 0 ? 0 : 3)} ${units[i]}`;
}

function fmtTime(secs) {
  if (!isFinite(secs) || secs < 0) return "—";
  secs = Math.round(secs);
  if (secs < 60) return `${secs} sec`;
  if (secs < 3600) return `${Math.floor(secs / 60)} min ${secs % 60} sec`;
  return `${Math.floor(secs / 3600)} hr ${Math.floor((secs % 3600) / 60)} min`;
}

function name(e) {
  return e.filename || e.title || decodeURIComponent(e.url.split("/").pop() || e.url);
}

function setTitle(text) {
  if (text !== lastTitle) {
    lastTitle = text;
    invoke("set_this_window_title", { title: text });
  }
}

function render() {
  const e = entry;
  if (!e) return;
  const pct = e.status === "completed" ? 100 : e.total ? (e.downloaded / e.total) * 100 : null;
  $("#url").textContent = e.url;
  $("#url").title = e.url;
  $("#size").textContent = e.total != null ? fmtBytes(e.total) : "Unknown";
  $("#downloaded").textContent = pct != null ? `${fmtBytes(e.downloaded)} (${pct.toFixed(2)} %)` : fmtBytes(e.downloaded);
  $("#bar").style.width = `${pct ?? 0}%`;
  const running = e.status === "downloading";
  $("#rate").textContent = running && e.speed > 0 ? `${fmtBytes(e.speed)}/sec` : "—";
  $("#eta").textContent = running && e.total && e.speed > 0 ? fmtTime((e.total - e.downloaded) / e.speed) : "—";
  $("#resume").textContent = e.kind === "media" || e.resumable ? "Yes" : e.total == null && e.downloaded === 0 ? "—" : "No";

  const status = {
    queued: "Waiting in the queue…",
    downloading: e.stage ? `${e.stage}…` : e.downloaded ? "Receiving data…" : "Connecting…",
    paused: "Paused",
    scheduled: "Waiting for the scheduler",
    completed: "Download complete",
    failed: `Failed: ${e.error || "unknown error"}`,
  }[e.status];
  $("#status").textContent = status;
  $("#status").title = status;

  const done = e.status === "completed";
  if (done && detailsOpen) {
    // Finished: the connection view is no longer interesting.
    detailsOpen = false;
    $("#details").hidden = true;
    invoke("resize_this_window", { width: 560, height: 270 });
  }
  $("#running-buttons").hidden = done;
  $("#done-buttons").hidden = !done;
  $("#root").classList.toggle("done", done);
  $("#pause").textContent = e.status === "downloading" || e.status === "queued" ? "Pause" : "Resume";
  setTitle(done ? `Download complete · ${name(e)}` : pct != null ? `${Math.floor(pct)}% ${name(e)}` : name(e));
  if (detailsOpen) renderParts();
}

function renderParts() {
  const e = entry;
  const parts = e.parts || [];
  const total = e.total || 0;
  const bar = $("#parts");
  bar.replaceChildren();
  const rows = $("#part-rows");
  rows.replaceChildren();
  if (!parts.length || !total) {
    const tr = document.createElement("tr");
    tr.innerHTML = `<td class="num">1</td><td class="num"></td><td></td>`;
    tr.children[1].textContent = fmtBytes(e.downloaded);
    tr.children[2].textContent = e.kind === "media" ? "Video download (yt-dlp)" : e.status === "downloading" ? "Receiving data…" : "";
    rows.appendChild(tr);
    return;
  }
  parts.forEach(([start, end, got], i) => {
    const fill = document.createElement("span");
    fill.style.left = `${(start / total) * 100}%`;
    fill.style.width = `${(got / total) * 100}%`;
    bar.appendChild(fill);
    const mark = document.createElement("i");
    mark.style.left = `${((start + got) / total) * 100}%`;
    if (got < end - start) bar.appendChild(mark);
    const tr = document.createElement("tr");
    tr.innerHTML = `<td class="num"></td><td class="num"></td><td></td>`;
    tr.children[0].textContent = i + 1;
    tr.children[1].textContent = fmtBytes(got);
    tr.children[2].textContent = got >= end - start ? "Finished" : e.status === "downloading" ? "Receiving data…" : "Paused";
    rows.appendChild(tr);
  });
}

$("#details-toggle").onclick = () => {
  detailsOpen = !detailsOpen;
  $("#details").hidden = !detailsOpen;
  $("#details-toggle").innerHTML = detailsOpen ? "&lt;&lt; Hide details" : "Show details &gt;&gt;";
  invoke("resize_this_window", { width: 560, height: detailsOpen ? 500 : 270 });
  render();
};
$("#pause").onclick = () => {
  const running = entry.status === "downloading" || entry.status === "queued";
  invoke(running ? "pause" : "resume", { ids: [id] });
};
$("#cancel").onclick = async () => {
  if (entry.status === "downloading" || entry.status === "queued") await invoke("pause", { ids: [id] });
  invoke("close_this_window");
};
$("#close").onclick = () => invoke("close_this_window");
$("#open").onclick = async () => { await invoke("open_file", { path: entry.path }).catch(() => {}); invoke("close_this_window"); };
$("#folder").onclick = async () => { await invoke("show_in_folder", { path: entry.path }).catch(() => {}); invoke("close_this_window"); };
document.addEventListener("contextmenu", (ev) => ev.preventDefault());

(async () => {
  await listen("downloads", (ev) => {
    const c = ev.payload;
    if (c.kind === "updated" && c.data.id === id) entry = c.data;
    else if (c.kind === "list") {
      const found = c.data.find((x) => x.id === id);
      if (!found) { invoke("close_this_window"); return; }
      // List snapshots don't carry live speed; keep the last one while running.
      entry = { ...found, parts: found.parts?.length ? found.parts : entry?.parts || [] };
    } else return;
    render();
  });
  entry = await invoke("get_entry", { id });
  if (!entry) { invoke("close_this_window"); return; }
  render();
})();
