// Video sites handled by yt-dlp. Kept in sync with MEDIA_SITES in
// crates/engine/src/media.rs (a test checks this).
const MEDIA_SITES = [
  "youtube.com", "youtu.be", "youtube-nocookie.com", "vimeo.com", "dailymotion.com",
  "twitch.tv", "x.com", "twitter.com", "facebook.com", "fb.watch", "instagram.com",
  "tiktok.com", "reddit.com", "soundcloud.com", "bandcamp.com", "bilibili.com",
  "ted.com", "streamable.com", "rumble.com", "kick.com", "odysee.com", "mixcloud.com",
  "9gag.com", "imgur.com", "pinterest.com", "linkedin.com", "threads.net"
];

/** Whether a page on this address is best downloaded with yt-dlp. */
function isMediaSite(url) {
  let host;
  try {
    host = new URL(url).hostname.replace(/^(www|m)\./, "");
  } catch {
    return false;
  }
  return MEDIA_SITES.some((site) => host === site || host.endsWith(`.${site}`));
}
