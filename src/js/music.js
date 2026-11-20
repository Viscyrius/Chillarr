// Music, laid out like a streaming app: Home (greeting, quick picks, chips,
// rows made from what you play, the Top 10, trending, new albums), Library
// (Liked songs, Downloaded, Recently played), artist and album pages, search
// with filters, a player bar and a full screen Now playing with the queue.
// What there is comes from the iTunes catalog; the sound is fetched on the PC
// (music.rs) and streamed from the bridge. Play: fetched, played and deleted
// again later. Download: kept in Music\Chillarr.
import { $, ask, el, icon, invoke, toast, WEB, viewer, fmtTime, getPref, setPref } from "./api.js";

const I = {
  play: "M7 4.5v15l12-7.5z",
  pause: "M7 4h3.5v16H7zM13.5 4H17v16h-3.5z",
  prev: "M6 5v14M19 5 9 12l10 7z",
  next: "M18 5v14M5 5l10 7-10 7z",
  down: "M12 4v11M7 10l5 5 5-5M5 20h14",
  done: "M5 12.5l4.5 4.5L19 7",
  trash: "M4 7h16M10 11v6M14 11v6M6 7l1 13h10l1-13M9 7V4h6v3",
  close: "M6 6l12 12M18 6 6 18",
  vol: "M4 9.5h4L13 5v14l-5-4.5H4zM16.5 8.5a5 5 0 0 1 0 7M19 6a8.5 8.5 0 0 1 0 12",
  mute: "M4 9.5h4L13 5v14l-5-4.5H4zM17 9.5l5 5M22 9.5l-5 5",
  shuffle: "M4 7h3.5c2.5 0 3.6 1.4 5 4.5S15 16.5 17.5 16.5H20M17 13.5l3 3-3 3M4 16.5h3.5c1.2 0 2-.3 2.8-1M13.7 8.5c.8-.7 1.7-1 3.8-1H20M17 4.5l3 3-3 3",
  repeat: "M17 2l3 3-3 3M4 11V9a4 4 0 0 1 4-4h12M7 22l-3-3 3-3M20 13v2a4 4 0 0 1-4 4H4",
  heart: "M12 20.5S3.5 15.2 3.5 9.1A4.6 4.6 0 0 1 12 6.6a4.6 4.6 0 0 1 8.5 2.5c0 6.1-8.5 11.4-8.5 11.4z",
  queue: "M4 6h12M4 12h12M4 18h7M17 14v7l5-3.5z",
  chevron: "M6 9l6 6 6-6",
  clock: "M12 7v5l3 2M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18z",
  note: "M9 18V5l11-2v13M9 18a3 3 0 1 1-6 0 3 3 0 0 1 6 0zM20 16a3 3 0 1 1-6 0 3 3 0 0 1 6 0z",
  add: "M12 5v14M5 12h14",
  addto: "M3 6h12M3 12h12M3 18h7M18 13v8M14 17h8",
  list: "M8 6h13M8 12h13M8 18h13M3 6h.01M3 12h.01M3 18h.01",
  pen: "M4 20h4L19 9l-4-4L4 16zM14 6l4 4",
  expand: "M15 3h6v6M9 21H3v-6M21 3l-7 7M3 21l7-7",
  more: "M5 12h.01M12 12h.01M19 12h.01",
  mic: "M12 15a3 3 0 0 0 3-3V6a3 3 0 0 0-6 0v6a3 3 0 0 0 3 3zM19 11a7 7 0 0 1-14 0M12 18v3M8 21h8",
  user: "M20 21v-1a5 5 0 0 0-5-5H9a5 5 0 0 0-5 5v1M12 11a4 4 0 1 0 0-8 4 4 0 0 0 0 8z",
  disc: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM12 14a2 2 0 1 0 0-4 2 2 0 0 0 0 4z",
  yt: "M22 8.5a3 3 0 0 0-2.1-2.1C18 6 12 6 12 6s-6 0-7.9.4A3 3 0 0 0 2 8.5 31 31 0 0 0 1.7 12 31 31 0 0 0 2 15.5a3 3 0 0 0 2.1 2.1C6 18 12 18 12 18s6 0 7.9-.4a3 3 0 0 0 2.1-2.1 31 31 0 0 0 .3-3.5 31 31 0 0 0-.3-3.5zM10 15V9l5 3z",
};
// A playlist made, renamed or changed: a Library or Home page on screen shows it.
window.addEventListener("chillarr:music-lists", () => {
  const h = location.hash;
  if (document.body.dataset.mode === "music" && h === "#m-playlists") window.dispatchEvent(new Event("hashchange"));
});
const ic = (k, cls) => { const s = icon(I[k]); if (cls) s.classList.add(cls); return s; };
const err = (e) => String(e?.message || e);
// Covers come from Apple at whatever size is asked for (and YouTube in two):
// the 600px one only where it is shown big. A phone listing forty songs
// fetched forty big covers before; a row's 42px picture now costs a fifth.
const thumb = (u, px) => !u ? u
  : /\/\d+x\d+bb\.(jpg|png|webp)$/.test(u) ? u.replace(/\/\d+x\d+bb\.\w+$/, `/${px}x${px}bb.jpg`)
  : px <= 200 && u.includes("i.ytimg.com/") ? u.replace("/hqdefault.", "/mqdefault.") : u;
// A small, quick settle-in for whatever just changed (a new song, a panel);
// nothing for someone who asked for less motion.
const calm = matchMedia("(prefers-reduced-motion: reduce)");
function settle(node, from = "translateY(6px)", ms = 260) {
  if (!node || calm.matches || !node.animate) return;
  node.animate([{ opacity: 0, transform: from }, { opacity: 1, transform: "none" }], { duration: ms, easing: "cubic-bezier(0.22, 1, 0.36, 1)" });
}
const img = (u, px, cls) => el("img", { class: cls, src: thumb(u, px), alt: "", loading: "lazy", decoding: "async" });

/* ---------- what this profile has: downloads, likes, plays ---------- */
let keptIds = new Set();
let keptLoaded = null;
function loadKept(force = false) {
  if (!keptLoaded || force) keptLoaded = invoke("music_library").then((l) => { keptIds = new Set(l.map((s) => s.id)); return l; }).catch(() => []);
  return keptLoaded;
}
const fetching = new Set();
let mine = { liked: [], recent: [], plays: {} };
let likedIds = new Set();
let mineLoaded = null;
function loadMine(force = false) {
  if (!mineLoaded || force) mineLoaded = invoke("music_mine").then((m) => { mine = m; likedIds = new Set(m.liked.map((s) => s.id)); return m; }).catch(() => mine);
  return mineLoaded;
}

async function toggleLike(song) {
  const on = !likedIds.has(song.id);
  on ? likedIds.add(song.id) : likedIds.delete(song.id);
  paintLikes(song.id);
  try {
    await invoke("music_like", { song, on });
    if (on) mine.liked.unshift(song); else mine.liked = mine.liked.filter((s) => s.id !== song.id);
    toast(on ? "Added to Liked songs" : "Removed from Liked songs");
  } catch (e) {
    on ? likedIds.delete(song.id) : likedIds.add(song.id);
    paintLikes(song.id);
    toast(err(e), true);
  }
}
function paintLike(b) {
  const on = likedIds.has(b.dataset.id);
  b.classList.toggle("on", on);
  b.title = on ? "Remove from Liked songs" : "Save to Liked songs";
  b.setAttribute("aria-label", b.title);
  b.setAttribute("aria-pressed", String(on));
}
function paintLikes(id) { document.querySelectorAll(`.mu-like[data-id="${CSS.escape(id)}"]`).forEach(paintLike); }
function likeBtn(song) {
  const b = el("button", { class: "mu-like mu-ib", dataset: { id: song.id }, onclick: (e) => { e.stopPropagation(); toggleLike(song); } }, ic("heart"));
  paintLike(b);
  return b;
}

async function keep(song) {
  if (keptIds.has(song.id) || fetching.has(song.id)) return;
  fetching.add(song.id);
  markKeep(song.id);
  try {
    await invoke("music_keep", { song });
    keptIds.add(song.id);
    keptLoaded = null;
    toast(`Downloaded ${song.title}`);
  } catch (e) {
    toast(err(e), true);
  } finally {
    fetching.delete(song.id);
    markKeep(song.id);
  }
}
function markKeep(id) { document.querySelectorAll(`.mu-keep[data-id="${CSS.escape(id)}"]`).forEach(paintKeep); }
function paintKeep(b) {
  const id = b.dataset.id;
  const st = keptIds.has(id) ? "done" : fetching.has(id) ? "busy" : "";
  b.dataset.state = st;
  b.title = st === "done" ? "Downloaded" : st === "busy" ? "Downloading…" : "Download";
  b.setAttribute("aria-label", b.title);
  b.replaceChildren(ic(st === "done" ? "done" : "down"));
}
function keepBtn(song) {
  const b = el("button", { class: "mu-keep mu-ib", dataset: { id: song.id }, onclick: (e) => { e.stopPropagation(); keep(song); } });
  paintKeep(b);
  return b;
}

/* ---------- playlists ---------- */
const lists = () => mine.playlists || [];
let menu = null;
function closeMenu() { if (!menu) return; const m = menu; menu = null; m.classList.remove("in"); setTimeout(() => m.remove(), 160); }
document.addEventListener("pointerdown", (e) => { if (menu && !menu.contains(e.target)) closeMenu(); }, true);
window.addEventListener("hashchange", closeMenu);
// Only a change of width closes it: a phone's keyboard coming up changes the height.
let menuW = innerWidth;
window.addEventListener("resize", () => { if (innerWidth !== menuW) { menuW = innerWidth; closeMenu(); } });
document.addEventListener("keydown", (e) => { if (e.key === "Escape") closeMenu(); });

/** A small menu by `anchor`. On a phone it comes up from the bottom. */
function popMenu(anchor, kids) {
  closeMenu();
  menu = el("div", { class: "look-pop mu-menu", role: "menu" }, kids);
  document.body.append(menu);
  const r = anchor.getBoundingClientRect();
  if (matchMedia("(max-width: 640px)").matches) menu.classList.add("sheet");
  else {
    const w = 260;
    menu.style.width = `${w}px`;
    const h = menu.offsetHeight;
    const below = r.bottom + 8 + h < innerHeight - 8;
    menu.style.top = `${Math.round(below ? r.bottom + 8 : Math.max(8, r.top - 8 - h))}px`;
    menu.style.left = `${Math.round(Math.min(Math.max(8, r.right - w), innerWidth - 8 - w))}px`;
  }
  requestAnimationFrame(() => menu && menu.classList.add("in"));
  return menu;
}

/** "Add to playlist": every playlist, and a new one named right here. */
function addToMenu(anchor, song) {
  const name = el("input", { type: "text", class: "mu-name", placeholder: "New playlist", maxlength: "100", "aria-label": "New playlist's name" });
  const make = async () => {
    const n = name.value.trim() || `My playlist #${lists().length + 1}`;
    try {
      const p = await invoke("music_playlist_new", { name: n, song });
      mine.playlists = [p, ...lists()];
      toast(`Added to ${p.name}`);
      closeMenu();
      refreshLists();
    } catch (e) { toast(err(e), true); }
  };
  name.addEventListener("keydown", (e) => { if (e.key === "Enter") make(); });
  popMenu(anchor, [
    el("p", { class: "lk-h", text: "Add to playlist" }),
    el("div", { class: "mu-new" }, name, el("button", { class: "mu-ib", "aria-label": "Make it", title: "New playlist", onclick: make }, ic("add"))),
    el("div", { class: "mu-menu-list" }, lists().map((p) => el("button", { class: "mu-menu-item", role: "menuitem", onclick: async () => {
      closeMenu();
      try {
        const added = await invoke("music_playlist_add", { id: p.id, song });
        if (added) { p.songs.push(song); toast(`Added to ${p.name}`); refreshLists(); } else toast(`Already in ${p.name}`);
      } catch (e) { toast(err(e), true); }
    } }, cover4(p.songs, "sm"), el("span", { class: "mu-tt" }, el("b", { text: p.name }), el("span", { text: `${p.songs.length} songs` }))))),
  ]);
  if (!matchMedia("(pointer: coarse)").matches) name.focus();
}
/** Puts a song at the end of what's playing. */
function addToQueue(song) {
  if (P.at < 0) { playList([song], 0, "Queue"); return; }
  P.queue.push(song);
  toast(`${song.title} added to the queue`);
  drawQueue();
}

/** Everything a song can do, without playing it first: the "⋯" button, a
    right click, or a long press on a phone. `extra` adds the page's own. */
function songMenu(anchor, song, extra = []) {
  const item = (icn, label, fn, cls = "") => el("button", { class: `mu-menu-item mu-mi ${cls}`, role: "menuitem", onclick: () => { closeMenu(); fn(); } }, ic(icn), el("span", { text: label }));
  const liked = likedIds.has(song.id);
  const kept = keptIds.has(song.id);
  popMenu(anchor, [
    el("div", { class: "mu-menu-head" }, img(song.art, 120), el("span", { class: "mu-tt" }, el("b", { text: song.title }), el("span", { text: song.artist }))),
    item("addto", "Add to playlist", () => addToMenu(anchor, song)),
    item("heart", liked ? "Remove from Liked songs" : "Save to Liked songs", () => toggleLike(song), liked ? "on fill" : ""),
    item("queue", "Play next", () => playNext(song)),
    item("list", "Add to queue", () => addToQueue(song)),
    kept ? item("done", "Downloaded", () => {}, "on") : item("down", fetching.has(song.id) ? "Downloading…" : "Download", () => keep(song)),
    song.artist_id ? item("user", "Go to artist", () => (location.hash = `#m-artist/${song.artist_id}`)) : null,
    song.album_id ? item("disc", "Go to album", () => (location.hash = `#m-album/${song.album_id}`)) : null,
    ...extra.map(([icn, label, fn]) => item(icn, label, fn)),
  ]);
}
function moreBtn(song, extra) {
  return el("button", { class: "mu-ib mu-more-btn", title: "More", "aria-label": `More for ${song.title}`, "aria-haspopup": "menu", onclick: (e) => { e.stopPropagation(); e.preventDefault(); songMenu(e.currentTarget, song, extra); } }, ic("more"));
}
/** A right click or a long press on anything that is a song opens its menu. */
function withMenu(node, song, extra) {
  node.addEventListener("contextmenu", (e) => { e.preventDefault(); songMenu(node.querySelector(".mu-more-btn") || node, song, extra); });
  return node;
}

function addBtn(song) {
  return el("button", { class: "mu-ib mu-addto", title: "Add to playlist", "aria-label": "Add to playlist", onclick: (e) => { e.stopPropagation(); addToMenu(e.currentTarget, song); } }, ic("addto"));
}
/** A playlist's cover: its first four songs' art, or one, or a note. */
function cover4(songs, cls = "") {
  const arts = [...new Set(songs.map((s) => s.art).filter(Boolean))].slice(0, 4);
  const box = el("span", { class: `mu-mosaic ${cls}${arts.length >= 4 ? " four" : ""}` });
  if (arts.length >= 4) arts.forEach((a) => box.append(img(a, 160)));
  else if (arts.length) box.append(img(arts[0], 300));
  else box.append(ic("note"));
  return box;
}
/** A page showing playlists is drawn again when one changes. */
function refreshLists() { window.dispatchEvent(new Event("chillarr:music-lists")); }

/* ---------- the player ---------- */
const audio = new Audio();
audio.preload = "auto";
const P = { queue: [], at: -1, ready: new Map(), seq: 0, from: "", shuffle: getPref("music-shuffle") === "1", repeat: getPref("music-repeat") || "off", counted: "", radio: false };
let base = null;

async function bridgeBase() {
  if (!base) base = WEB ? await window.__CHILLARR_BRIDGE__() : await invoke("read_base");
  return base;
}

/** The song made ready on the PC (fetched unless downloaded): the URL to play. */
function ready(song) {
  const have = P.ready.get(song.id);
  if (have) return have;
  const p = Promise.all([bridgeBase(), invoke("music_play", { song })]).then(([b, r]) => `${b}${r.path}`);
  P.ready.set(song.id, p);
  p.catch(() => P.ready.delete(song.id));
  if (P.ready.size > 60) P.ready.delete(P.ready.keys().next().value);
  return p;
}

/* Phones (iPhones above all) only let a page start sound straight from a tap.
   A song takes seconds to fetch, so the tap itself starts a moment of silence
   on the player: from then on the same player may play whatever it is given. */
let unlocked = false;
let quiet = "";
function silence() {
  if (quiet) return quiet;
  const n = 800, b = new Uint8Array(44 + n), v = new DataView(b.buffer);
  const s = (o, t) => [...t].forEach((c, i) => (b[o + i] = c.charCodeAt(0)));
  s(0, "RIFF"); v.setUint32(4, 36 + n, true); s(8, "WAVEfmt "); v.setUint32(16, 16, true); v.setUint16(20, 1, true); v.setUint16(22, 1, true);
  v.setUint32(24, 8000, true); v.setUint32(28, 8000, true); v.setUint16(32, 1, true); v.setUint16(34, 8, true); s(36, "data"); v.setUint32(40, n, true);
  b.fill(128, 44);
  quiet = URL.createObjectURL(new Blob([b], { type: "audio/wav" }));
  return quiet;
}
function unlock() {
  if (unlocked) return;
  unlocked = true;
  try {
    audio.src = silence();
    const p = audio.play();
    if (p) p.then(() => audio.pause()).catch(() => { unlocked = false; });
  } catch (e) { unlocked = false; }
}

function shuffled(a) { const b = a.slice(); for (let i = b.length - 1; i > 0; i--) { const j = Math.floor(Math.random() * (i + 1)); [b[i], b[j]] = [b[j], b[i]]; } return b; }

/** Plays `list[i]`, with the rest of `list` as what comes next. `from` names the list. */
export function playList(list, i = 0, from = "") {
  const pick = list[i];
  if (!pick) return;
  unlock();
  P.from = from;
  P.radio = false;
  if (P.shuffle) {
    P.queue = [pick, ...shuffled(list.filter((_, k) => k !== i))];
    start(0);
  } else {
    P.queue = list.slice();
    start(i);
  }
}
/** Puts a song right after the one playing. */
function playNext(song) {
  if (P.at < 0) { playList([song], 0, "Queue"); return; }
  P.queue.splice(P.at + 1, 0, song);
  toast(`${song.title} plays next`);
  drawQueue();
}

async function start(i) {
  if (i < 0 || i >= P.queue.length) return;
  P.at = i;
  const song = P.queue[i];
  const seq = ++P.seq;
  P.counted = "";
  document.querySelectorAll(".mu-seek").forEach((s) => { delete s.dataset.held; s.value = "0"; rangeFill(s); });
  drawBar(true);
  audio.pause();
  try {
    const url = await ready(song);
    if (seq !== P.seq) return;
    audio.src = url;
    await audio.play().catch(() => {});
    drawBar(false);
    media(song);
    // The next one is fetched while this one plays, so it starts at once.
    const nx = P.queue[i + 1];
    if (nx) { ready(nx).catch(() => {}); lyricsFor(nx); }
    else topUp();
    lyricsFor(song);
  } catch (e) {
    if (seq !== P.seq) return;
    drawBar(false);
    toast(err(e), true);
    // A song that can't be found: on to the next, not stuck.
    if (P.queue[i + 1]) setTimeout(() => { if (seq === P.seq) start(i + 1); }, 1200);
  }
}

/** The list ran out: songs like the last one keep it going (radio). */
async function topUp() {
  const last = P.queue[P.queue.length - 1];
  if (!last || P.repeat !== "off") return;
  try {
    const more = await invoke("music_radio", { song: last, skip: P.queue.slice(-50).map((s) => s.id) });
    if (!more.length || P.queue[P.queue.length - 1] !== last) return;
    P.radio = true;
    P.queue.push(...more);
    drawQueue();
    const nx = P.queue[P.at + 1];
    if (nx) ready(nx).catch(() => {});
  } catch (e) { /* the list just ends */ }
}

function next() {
  if (P.repeat === "all" && P.at + 1 >= P.queue.length) return start(0);
  return start(P.at + 1);
}
const prev = () => (audio.currentTime > 4 ? (audio.currentTime = 0) : start(Math.max(0, P.at - 1)));
const toggle = () => { if (!audio.src || P.at < 0) return; audio.paused ? audio.play().catch(() => {}) : audio.pause(); };

/** The moment of silence that unlocks a phone's player isn't a song. */
const silent = () => audio.src.startsWith("blob:");
audio.addEventListener("ended", () => {
  if (silent()) return;
  if (P.repeat === "one") { audio.currentTime = 0; audio.play().catch(() => {}); return; }
  if (P.at + 1 < P.queue.length || P.repeat === "all") next(); else drawBar(false);
});
for (const ev of ["play", "pause"]) audio.addEventListener(ev, () => { if (!silent()) drawBar(false); });
audio.addEventListener("timeupdate", () => {
  if (silent()) return;
  drawTime();
  // Counts as played after 20 seconds (or half a short song), once.
  const s = P.queue[P.at];
  if (s && P.counted !== s.id && audio.currentTime > Math.min(20, (audio.duration || 40) / 2)) {
    P.counted = s.id;
    invoke("music_played", { song: s }).then(() => { mine.recent = [s, ...mine.recent.filter((x) => x.id !== s.id)]; }).catch(() => {});
  }
});
audio.addEventListener("loadedmetadata", () => drawTime());

function media(s) {
  if (!("mediaSession" in navigator)) return;
  navigator.mediaSession.metadata = new MediaMetadata({ title: s.title, artist: s.artist, album: s.album, artwork: s.art ? [{ src: s.art, sizes: "600x600", type: "image/jpeg" }] : [] });
  navigator.mediaSession.setActionHandler("play", () => audio.play());
  navigator.mediaSession.setActionHandler("pause", () => audio.pause());
  navigator.mediaSession.setActionHandler("nexttrack", next);
  navigator.mediaSession.setActionHandler("previoustrack", prev);
  try { navigator.mediaSession.setActionHandler("seekto", (d) => { if (d.seekTime != null) audio.currentTime = d.seekTime; }); } catch (e) { /* not on this browser */ }
}
/** The phone's lock screen shows where the song is. */
function lockPosition() {
  if (!("mediaSession" in navigator) || !navigator.mediaSession.setPositionState || !isFinite(audio.duration)) return;
  try { navigator.mediaSession.setPositionState({ duration: audio.duration, position: Math.min(audio.currentTime, audio.duration), playbackRate: 1 }); } catch (e) { /* not on this browser */ }
}
audio.addEventListener("loadedmetadata", lockPosition);
audio.addEventListener("seeked", lockPosition);
audio.addEventListener("play", () => { if ("mediaSession" in navigator) navigator.mediaSession.playbackState = "playing"; lockPosition(); });
audio.addEventListener("pause", () => { if ("mediaSession" in navigator) navigator.mediaSession.playbackState = "paused"; });

/* ---------- the bar ---------- */
let bar = null;
function rangeFill(r) { r.style.setProperty("--fill", `${(r.value / r.max) * 100}%`); }
function seekBar() {
  const seek = el("input", { type: "range", class: "mu-range mu-seek", min: "0", max: "1000", value: "0", "aria-label": "Position" });
  // Moves while it is dragged, not on release.
  // While held it shows where it's dragged to; let go, it follows the song again
  // (it used to wait for the focus to leave it, so it stayed where it was dropped).
  seek.addEventListener("input", () => { seek.dataset.held = "1"; rangeFill(seek); if (audio.duration) audio.currentTime = (seek.value / 1000) * audio.duration; });
  const letGo = () => { delete seek.dataset.held; drawTime(); };
  seek.addEventListener("change", letGo);
  seek.addEventListener("pointerup", letGo);
  seek.addEventListener("pointercancel", letGo);
  seek.addEventListener("blur", letGo);
  return seek;
}
const ctlButtons = () => [
  el("button", { class: "mu-ib mu-shuf", "aria-label": "Shuffle", title: "Shuffle", onclick: toggleShuffle }, ic("shuffle")),
  el("button", { class: "mu-ib mu-prev", "aria-label": "Previous", title: "Previous", onclick: prev }, ic("prev", "fill")),
  el("button", { class: "mu-pp", "aria-label": "Play", onclick: toggle }, ic("play")),
  el("button", { class: "mu-ib", "aria-label": "Next", title: "Next", onclick: next }, ic("next", "fill")),
  el("button", { class: "mu-ib mu-rep", "aria-label": "Repeat", title: "Repeat", onclick: cycleRepeat }, ic("repeat"), el("i", { class: "mu-one", text: "1" })),
];
function makeBar() {
  if (bar) return bar;
  const vol = el("input", { type: "range", class: "mu-range mu-vol", min: "0", max: "100", value: String(Math.round(Number(getPref("chillarr-music-vol") ?? 0.8) * 100)), "aria-label": "Volume" });
  audio.volume = vol.value / 100;
  rangeFill(vol);
  const volIc = el("button", { class: "mu-ib mu-volic", "aria-label": "Mute", onclick: () => { audio.muted = !audio.muted; volIc.replaceChildren(ic(audio.muted ? "mute" : "vol")); } }, ic("vol"));
  vol.addEventListener("input", () => { audio.volume = vol.value / 100; audio.muted = false; volIc.replaceChildren(ic("vol")); rangeFill(vol); });
  vol.addEventListener("change", () => setPref("chillarr-music-vol", String(audio.volume)));
  bar = el("section", { class: "mu-bar", id: "mu-bar", "aria-label": "Music player", hidden: true },
    el("div", { class: "mu-now" },
      el("button", { class: "mu-art-btn", "aria-label": "Now playing", title: "Now playing", onclick: () => { if (bar.classList.contains("min")) setMin(false); else openFull(); } }, el("img", { class: "mu-art", alt: "" })),
      el("div", { class: "mu-txt", onclick: () => { if (matchMedia("(max-width: 980px)").matches) openFull(); } }, el("b", { class: "mu-title" }), el("a", { class: "mu-artist" })),
      el("span", { class: "mu-like-slot" })),
    el("div", { class: "mu-mid" },
      el("div", { class: "mu-ctl" }, ctlButtons(), el("button", { class: "mu-ib mu-ly-mini", "aria-label": "Lyrics", title: "Lyrics", onclick: (e) => { e.stopPropagation(); openFull("lyrics"); } }, ic("mic"))),
      el("div", { class: "mu-line" }, el("span", { class: "mu-t0", text: "0:00" }), seekBar(), el("span", { class: "mu-t1", text: "0:00" }))),
    el("div", { class: "mu-side" },
      el("span", { class: "mu-keep-slot" }),
      el("button", { class: "mu-ib mu-lybtn", "aria-label": "Lyrics", title: "Lyrics", onclick: () => openFull("lyrics") }, ic("mic")),
      el("button", { class: "mu-ib mu-qbtn", "aria-label": "Up next", title: "Up next", onclick: () => openFull("queue") }, ic("list")),
      volIc, vol,
      el("button", { class: "mu-ib mu-min", "aria-label": "Fold away", title: "Fold away", onclick: () => setMin(true) }, icon("M5 12h14")),
      el("button", { class: "mu-ib mu-stop", "aria-label": "Stop", title: "Stop", onclick: stopAll }, ic("close"))),
    el("div", { class: "mu-mini-line" }));
  document.body.append(bar);
  paintModes();
  return bar;
}

/** The corner player (in Movies and Reading) folded to its cover, or open. */
function setMin(on) {
  bar.classList.toggle("min", on);
  setPref("music-min", on ? "1" : "0");
}
// Folding is only for the corner player: back in Music it's the full bar.
new MutationObserver(() => { if (bar) bar.classList.toggle("min", document.body.dataset.mode !== "music" && getPref("music-min") === "1"); })
  .observe(document.body, { attributes: true, attributeFilter: ["data-mode"] });

function toggleShuffle() {
  P.shuffle = !P.shuffle;
  setPref("music-shuffle", P.shuffle ? "1" : "0");
  if (P.at >= 0) {
    const rest = P.queue.slice(P.at + 1);
    P.queue = [...P.queue.slice(0, P.at + 1), ...(P.shuffle ? shuffled(rest) : rest)];
  }
  paintModes();
  drawQueue();
}
function cycleRepeat() {
  P.repeat = P.repeat === "off" ? "all" : P.repeat === "all" ? "one" : "off";
  setPref("music-repeat", P.repeat);
  paintModes();
}
function paintModes() {
  document.querySelectorAll(".mu-shuf").forEach((b) => { b.classList.toggle("on", P.shuffle); b.setAttribute("aria-pressed", String(P.shuffle)); });
  document.querySelectorAll(".mu-rep").forEach((b) => {
    b.classList.toggle("on", P.repeat !== "off");
    b.classList.toggle("one", P.repeat === "one");
    b.title = P.repeat === "off" ? "Repeat" : P.repeat === "all" ? "Repeat all" : "Repeat one";
  });
}

function stopAll() {
  P.seq++;
  audio.pause();
  audio.removeAttribute("src");
  audio.load();
  P.queue = [];
  P.at = -1;
  bar.hidden = true;
  closeFull();
  document.body.classList.remove("mu-on");
  markPlaying();
}

function drawBar(loading) {
  makeBar();
  const s = P.queue[P.at];
  if (!s) return;
  bar.hidden = false;
  document.body.classList.add("mu-on");
  bar.classList.toggle("loading", !!loading);
  bar.classList.toggle("spin", !audio.paused && !loading);
  if (document.body.dataset.mode !== "music" && getPref("music-min") === "1") bar.classList.add("min");
  const art = $(".mu-art", bar);
  const small = thumb(s.art, 160) || "logo.svg";
  const changed = art.getAttribute("src") !== small;
  if (changed) { art.src = small; bar.style.setProperty("--art", s.art ? `url("${small}")` : "none"); }
  $(".mu-title", bar).textContent = s.title;
  if (changed && art.getAttribute("data-shown")) { settle(art, "scale(0.9)"); settle($(".mu-txt", bar), "translateX(-8px)"); }
  art.setAttribute("data-shown", "1");
  const a = $(".mu-artist", bar);
  a.textContent = loading ? "Getting it ready…" : s.artist;
  if (s.artist_id && !loading) a.href = `#m-artist/${s.artist_id}`; else a.removeAttribute("href");
  const playing = !audio.paused && !loading;
  document.querySelectorAll(".mu-pp").forEach((pp) => { pp.replaceChildren(ic(playing ? "pause" : "play")); pp.setAttribute("aria-label", playing ? "Pause" : "Play"); pp.classList.toggle("busy", !!loading); });
  $(".mu-keep-slot", bar).replaceChildren(keepBtn(s));
  $(".mu-like-slot", bar).replaceChildren(likeBtn(s), addBtn(s));
  $(".mu-keep-slot", bar).append(moreBtn(s));
  markPlaying();
  drawTime();
  drawFull();
}

/** The song playing is marked wherever it is listed. */
function markPlaying() {
  const id = P.queue[P.at]?.id;
  const playing = !!id && !audio.paused;
  document.querySelectorAll(".mu-track").forEach((r) => { r.classList.toggle("on", r.dataset.id === id); r.classList.toggle("playing", r.dataset.id === id && playing); });
}

function drawTime() {
  if (!bar) return;
  const d = audio.duration || P.queue[P.at]?.secs || 0;
  const t = audio.currentTime || 0;
  document.querySelectorAll(".mu-t0").forEach((x) => (x.textContent = fmtTime(t)));
  document.querySelectorAll(".mu-t1").forEach((x) => (x.textContent = fmtTime(d)));
  document.querySelectorAll(".mu-seek").forEach((seek) => { if (!seek.dataset.held && d) { seek.value = String(Math.round((t / d) * 1000)); rangeFill(seek); } });
  const ml = $(".mu-mini-line", bar);
  if (ml) ml.style.width = d ? `${(t / d) * 100}%` : "0";
}

/* ---------- Now playing, full screen, with the queue ---------- */
let full = null;
function openFull(panel) {
  if (!P.queue[P.at]) return;
  if (!full) {
    full = el("section", { class: "mu-full", "aria-label": "Now playing", role: "dialog" },
      el("div", { class: "mu-full-bg" }),
      el("header", { class: "mu-full-top" },
        el("button", { class: "mu-ib", "aria-label": "Close", title: "Close", onclick: closeFull }, ic("chevron")),
        el("span", { class: "mu-full-from" }),
        el("button", { class: "mu-ib mu-full-ly", "aria-label": "Lyrics", title: "Lyrics", onclick: () => setPanel(full.dataset.panel === "lyrics" ? "queue" : "lyrics") }, ic("mic"), "Lyrics")),
      el("div", { class: "mu-full-body" },
        el("div", { class: "mu-full-np" },
          el("img", { class: "mu-full-art", alt: "" }),
          el("div", { class: "mu-full-meta" },
            el("div", {}, el("h2", { class: "mu-full-title" }), el("a", { class: "mu-full-artist" })),
            el("span", { class: "mu-full-acts" })),
          el("div", { class: "mu-line" }, el("span", { class: "mu-t0", text: "0:00" }), seekBar(), el("span", { class: "mu-t1", text: "0:00" })),
          el("div", { class: "mu-ctl big" }, ctlButtons())),
        el("aside", { class: "mu-full-q" },
          el("div", { class: "mu-ptabs", role: "tablist" },
            el("button", { class: "mu-ptab", role: "tab", dataset: { p: "lyrics" }, onclick: () => setPanel("lyrics") }, ic("mic"), "Lyrics"),
            el("button", { class: "mu-ptab", role: "tab", dataset: { p: "queue" }, onclick: () => setPanel("queue") }, ic("list"), "Up next")),
          el("div", { class: "mu-lyrics", role: "tabpanel", "aria-label": "Lyrics" }),
          el("ol", { class: "mu-qlist", role: "tabpanel", "aria-label": "Up next" }))));
    // Scrolling the lyrics yourself pauses following the song for a moment.
    const lb = $(".mu-lyrics", full);
    for (const ev of ["wheel", "touchmove"]) lb.addEventListener(ev, () => (L.userAt = Date.now()), { passive: true });
    full.addEventListener("click", (e) => { if (e.target.closest("a[href]")) closeFull(); });
    // On a phone, a swipe down from the top half closes it.
    let y0 = null, dy = 0;
    full.addEventListener("touchstart", (e) => { const t = e.touches[0]; y0 = t.clientY < innerHeight * 0.6 && !e.target.closest(".mu-full-q, .mu-lyrics, input") && $(".mu-full-body", full).scrollTop <= 0 ? t.clientY : null; dy = 0; }, { passive: true });
    full.addEventListener("touchmove", (e) => { if (y0 == null) return; dy = Math.max(0, e.touches[0].clientY - y0); full.style.transform = `translateY(${dy}px)`; full.style.transition = "none"; }, { passive: true });
    full.addEventListener("touchend", () => { if (y0 == null) return; full.style.transition = ""; full.style.transform = ""; if (dy > 110) closeFull(); y0 = null; });
    document.body.append(full);
    paintModes();
  }
  setPanel(panel || getPref("music-panel") || "lyrics");
  drawFull();
  drawBar(bar?.classList.contains("loading"));
  requestAnimationFrame(() => { full.classList.add("in"); drawLyricTime(true); });
  document.body.classList.add("mu-full-on");
}
function closeFull() {
  if (!full) return;
  full.classList.remove("in");
  document.body.classList.remove("mu-full-on");
}
function drawFull() {
  if (!full) return;
  const s = P.queue[P.at];
  if (!s) return;
  $(".mu-full-bg", full).style.backgroundImage = s.art ? `url("${s.art}")` : "";
  const art = $(".mu-full-art", full);
  const changed = art.getAttribute("src") !== (s.art || "logo.svg");
  if (changed) {
    art.src = s.art || "logo.svg";
    if (full.classList.contains("in")) { settle(art, "scale(0.94)", 340); settle($(".mu-full-meta", full), "translateY(8px)", 340); settle($(full.dataset.panel === "lyrics" ? ".mu-lyrics" : ".mu-qlist", full), "translateY(10px)", 320); }
  }
  $(".mu-full-title", full).textContent = s.title;
  const a = $(".mu-full-artist", full);
  a.textContent = s.artist;
  if (s.artist_id) a.href = `#m-artist/${s.artist_id}`; else a.removeAttribute("href");
  $(".mu-full-from", full).textContent = P.from ? `Playing from ${P.from}${P.radio ? ", then songs like it" : ""}` : "Now playing";
  $(".mu-full-acts", full).replaceChildren(likeBtn(s), addBtn(s), keepBtn(s));
  drawQueue();
  drawLyrics();
}

/* ---------- lyrics ---------- */
// L: the lyrics for the song playing. `rate` times them for a sped up or
// slowed version; `loose`: shown as text only (a remix, laid out otherwise).
const L = { id: "", data: null, rate: 1, loose: false, at: -1, userAt: 0, p: null };
const lyricCache = new Map();
function lyricsFor(s) {
  if (!lyricCache.has(s.id)) {
    const p = invoke("music_lyrics", { song: s }).catch(() => ({ found: false }));
    lyricCache.set(s.id, p);
    if (lyricCache.size > 200) lyricCache.delete(lyricCache.keys().next().value);
  }
  return lyricCache.get(s.id);
}
function setPanel(p) {
  if (!full) return;
  const was = full.dataset.panel;
  full.dataset.panel = p;
  if (was && was !== p && full.classList.contains("in")) {
    settle($(p === "lyrics" ? ".mu-lyrics" : ".mu-qlist", full), p === "lyrics" ? "translateX(-14px)" : "translateX(14px)");
    settle($(".mu-full-np", full), "translateY(4px)", 220);
  }
  setPref("music-panel", p);
  full.querySelectorAll(".mu-ptab").forEach((b) => { const on = b.dataset.p === p; b.classList.toggle("on", on); b.setAttribute("aria-selected", String(on)); });
  if (p === "lyrics") drawLyricTime(true);
}
async function drawLyrics() {
  if (!full) return;
  const s = P.queue[P.at];
  const box = $(".mu-lyrics", full);
  if (!s) return;
  if (L.id === s.id && L.data) return;
  L.id = s.id; L.data = null; L.at = -1;
  box.replaceChildren(el("div", { class: "mu-ly-wait" }, Array.from({ length: 6 }, () => el("i"))));
  const d = await lyricsFor(s);
  if (L.id !== s.id) return;
  L.data = d;
  const speedy = /sped|speed ?up|nightcore|slowed|\bslow\b|reverb/i.test(s.title);
  const ratio = audio.duration && d.duration ? audio.duration / d.duration : 1;
  L.rate = speedy && ratio > 0.5 && ratio < 1.8 ? ratio : 1;
  // A YouTube version that isn't the same song at another speed (a remix, a
  // mashup, live) is laid out differently: its words show, not timed.
  L.loose = String(s.id).startsWith("yt-") && !speedy && Math.abs(ratio - 1) > 0.06;
  if (!d.found) { box.replaceChildren(el("p", { class: "mu-ly-none", text: "No lyrics found for this one." })); return; }
  if (d.instrumental && !d.synced.length && !d.plain) { box.replaceChildren(el("p", { class: "mu-ly-none", text: "♪ Instrumental" })); return; }
  const note = d.matched ? el("p", { class: "mu-ly-note", text: `Lyrics of ${d.matched}${L.loose ? ", the original: this version's timing differs" : ""}` }) : null;
  if (d.synced.length && !L.loose) {
    box.replaceChildren(note, ...d.synced.map(([t, words], i) => el("p", { class: "mu-ly", dataset: { i: String(i) }, role: "button", tabindex: "0", title: "Play from here",
      onclick: () => seekLyric(t), onkeydown: (e) => { if (e.key === "Enter") seekLyric(t); } }, words || "♪")), el("p", { class: "mu-ly-src", text: "Lyrics from LRCLIB" }));
    box.classList.add("synced");
    drawLyricTime(true);
  } else {
    const text = d.plain || d.synced.map(([, w]) => w).join("\n");
    box.classList.remove("synced");
    box.replaceChildren(note, ...text.split("\n").map((w) => el("p", { class: "mu-ly plain" }, w || "\u00a0")), el("p", { class: "mu-ly-src", text: "Lyrics from LRCLIB" }));
    box.scrollTop = 0;
  }
}
/** A line pressed: the song goes there. */
function seekLyric(t) {
  const at = t / L.rate;
  if (isFinite(audio.duration)) audio.currentTime = Math.max(0, Math.min(at, audio.duration - 0.25));
  if (audio.paused) audio.play().catch(() => {});
  L.userAt = 0;
  drawLyricTime(true);
}
/** The line being sung lights up and comes to the middle. */
function drawLyricTime(force = false) {
  if (!full || !L.data?.synced?.length || L.loose || full.dataset.panel !== "lyrics") return;
  const t = (audio.currentTime || 0) * L.rate + 0.25;
  const lines = L.data.synced;
  let i = -1;
  for (let k = 0; k < lines.length; k++) { if (lines[k][0] <= t) i = k; else break; }
  if (i === L.at && !force) return;
  L.at = i;
  const box = $(".mu-lyrics", full);
  box.querySelectorAll(".mu-ly").forEach((p) => { const k = +p.dataset.i; p.classList.toggle("on", k === i); p.classList.toggle("past", k < i); });
  const cur = box.querySelector(`.mu-ly[data-i="${Math.max(0, i)}"]`);
  // Follows the song, unless you scrolled the lyrics in the last few seconds.
  if (cur && full.classList.contains("in") && Date.now() - L.userAt > 3500) {
    box.scrollTo({ top: cur.offsetTop - box.clientHeight * 0.38, behavior: force ? "auto" : "smooth" });
  }
}
audio.addEventListener("timeupdate", () => drawLyricTime());
audio.addEventListener("seeked", () => drawLyricTime(true));
// The length is known once the song loads: timing for a sped up version.
audio.addEventListener("loadedmetadata", () => { if (L.data && L.id === P.queue[P.at]?.id) { L.data = null; L.id = ""; drawLyrics(); } });
function drawQueue() {
  if (!full) return;
  const list = P.queue.slice(P.at + 1, P.at + 51);
  $(".mu-qlist", full).replaceChildren(...(list.length ? list.map((s, k) => el("li", { class: "mu-q", onclick: () => start(P.at + 1 + k) },
    img(s.art, 120), el("span", { class: "mu-tt" }, el("b", { text: s.title }), el("span", { text: s.artist })),
    el("button", { class: "mu-ib", title: "Take out", "aria-label": "Take out of the queue", onclick: (e) => { e.stopPropagation(); P.queue.splice(P.at + 1 + k, 1); drawQueue(); } }, ic("close")))) : [el("li", { class: "mu-empty", text: "Nothing after this one yet. Songs like it follow on their own." })]));
}

window.addEventListener("keydown", (e) => {
  if (!audio.src || ["INPUT", "TEXTAREA", "SELECT"].includes(document.activeElement?.tagName)) return;
  if (e.key === "Escape" && full?.classList.contains("in")) { closeFull(); return; }
  if (document.body.dataset.mode !== "music") return;
  if (e.key === " " || e.key === "k") { e.preventDefault(); toggle(); }
  else if (e.shiftKey && e.key === "ArrowRight") next();
  else if (e.shiftKey && e.key === "ArrowLeft") prev();
});
window.addEventListener("hashchange", closeFull);

/* ---------- pieces of pages ---------- */
const playBadge = (onclick) => el("button", { class: "mu-play-badge", "aria-label": "Play", onclick: (e) => { e.preventDefault(); e.stopPropagation(); onclick(); } }, ic("play"));

function artistLink(s) {
  return s.artist_id
    ? el("a", { class: "mu-sub mu-alink", href: `#m-artist/${s.artist_id}`, text: s.artist, onclick: (e) => e.stopPropagation() })
    : el("span", { class: "mu-sub", text: s.artist });
}

const isYt = (s) => String(s.id).startsWith("yt-");
const views = (s) => (s.views >= 1e6 ? `${(s.views / 1e6).toFixed(1).replace(/\.0$/, "")}M views` : s.views >= 1e3 ? `${Math.round(s.views / 1e3)}K views` : s.views ? `${s.views} views` : "YouTube");

const songCard = (s, list, i, from) => withMenu(el("div", { class: `mu-card${isYt(s) ? " yt" : ""}`, role: "button", tabindex: "0", title: `${s.title} — ${s.artist}`, onclick: () => playList(list, i, from), onkeydown: (e) => { if (e.key === "Enter") playList(list, i, from); } },
  el("span", { class: "mu-cover" }, img(s.art, 320), playBadge(() => playList(list, i, from)), moreBtn(s)),
  el("b", { text: s.title }), artistLink(s)), s);

const albumCard = (a) => el("a", { class: "mu-card", href: `#m-album/${a.id}`, title: `${a.title} — ${a.artist}` },
  el("span", { class: "mu-cover" }, img(a.art, 320), playBadge(() => playAlbum(a.id, a.title))),
  el("b", { text: a.title }), el("span", { class: "mu-sub", text: [a.year, a.artist].filter(Boolean).join(" · ") }));

const artistCard = (a) => el("a", { class: "mu-card mu-artist-card", href: `#m-artist/${a.id}`, title: a.name },
  el("span", { class: "mu-cover round" }, img(a.art, 320), playBadge(() => playArtist(a.id, a.name))),
  el("b", { text: a.name }), el("span", { class: "mu-sub", text: "Artist" }));

/** A big numbered card for the Top 10. */
const topCard = (s, list, i) => withMenu(el("div", { class: "mu-top", role: "button", tabindex: "0", onclick: () => playList(list, i, "Top 10"), onkeydown: (e) => { if (e.key === "Enter") playList(list, i, "Top 10"); } },
  el("span", { class: "mu-top-n", text: String(i + 1) }),
  el("span", { class: "mu-cover" }, img(s.art, 320), playBadge(() => playList(list, i, "Top 10")), moreBtn(s)),
  el("span", { class: "mu-top-txt" }, el("b", { text: s.title }), artistLink(s))), s);

async function playAlbum(id, title) {
  try { const r = await invoke("music_album", { id }); playList(r.songs, 0, title || r.album.title); } catch (e) { toast(err(e), true); }
}
async function playArtist(id, name) {
  try { const r = await invoke("music_artist", { id }); playList(r.songs, 0, name || r.artist.name); } catch (e) { toast(err(e), true); }
}

/** A row that scrolls sideways, with arrows for a mouse. */
function row(title, kids, { note: sub, more, id } = {}) {
  const rail = el("div", { class: "mu-rail" }, kids);
  const go = (d) => rail.scrollBy({ left: d * rail.clientWidth * 0.85, behavior: "smooth" });
  return el("section", { class: "mu-row", dataset: { row: id || "" } },
    el("div", { class: "mu-row-head" },
      el("div", {}, el("h2", { text: title }), sub ? el("p", { class: "mu-row-note", text: sub }) : null),
      el("div", { class: "mu-row-acts" }, more || null,
        el("button", { class: "mu-ib mu-arrow", "aria-label": "Back", onclick: () => go(-1) }, icon("M15 5l-7 7 7 7")),
        el("button", { class: "mu-ib mu-arrow", "aria-label": "More", onclick: () => go(1) }, icon("M9 5l7 7-7 7")))),
    rail);
}

/** A table of songs: number (play on hover), title, album, like, download, play next, length. */
function trackList(list, { removable = false, numbered = true, album = true, from = "" } = {}) {
  const coarse = matchMedia("(pointer: coarse)").matches;
  return el("div", { class: `mu-table${album ? "" : " no-album"}` },
    el("div", { class: "mu-thead" }, el("span", { text: "#" }), el("span", { text: "Title" }), album ? el("span", { class: "mu-col-album", text: "Album" }) : null, el("span"), el("span", { class: "mu-col-len" }, ic("clock")), el("span")),
    el("ol", { class: "mu-tracks" }, list.map((s, i) => {
      const play = () => { if (P.queue[P.at]?.id === s.id) toggle(); else playList(list, i, from); };
      const r = el("li", { class: "mu-track", dataset: { id: s.id }, tabindex: "0", onkeydown: (e) => { if (e.key === "Enter") play(); } },
        el("span", { class: "mu-n" }, el("span", { class: "mu-n-num", text: numbered ? String(i + 1) : "" }), el("button", { class: "mu-n-play", "aria-label": `Play ${s.title}`, onclick: (e) => { e.stopPropagation(); play(); } }, ic("play")), el("span", { class: "mu-eq", "aria-hidden": "true" }, el("i"), el("i"), el("i"))),
        el("span", { class: "mu-ti" }, img(s.art, 120),
          el("span", { class: "mu-tt" }, el("b", { text: s.title }),
            el("span", { class: "mu-byline" }, keptIds.has(s.id) ? el("i", { class: "mu-dl-mark", title: "Downloaded" }, ic("down")) : null, isYt(s) ? el("i", { class: "mu-yt-mark", title: "From YouTube" }, "YT") : null, artistLink(s)))),
        album ? (s.album_id ? el("a", { class: "mu-col-album", href: `#m-album/${s.album_id}`, text: s.album, onclick: (e) => e.stopPropagation() }) : el("span", { class: "mu-col-album", text: isYt(s) ? views(s) : s.album })) : null,
        el("span", { class: "mu-row-btns" }, typeof removable === "function" ? removable(s) : removable ? removeBtn(s) : null, likeBtn(s)),
        el("span", { class: "mu-len", text: s.secs ? fmtTime(s.secs) : "" }),
        moreBtn(s));
      // A double click plays on a PC; one tap on a phone.
      r.addEventListener(coarse ? "click" : "dblclick", play);
      return withMenu(r, s);
    })));
}

function removeBtn(s) {
  if (WEB && !viewer.can_delete && !viewer.admin) return el("span");
  return el("button", { class: "mu-ib", title: "Delete download", "aria-label": "Delete download", onclick: async (e) => {
    e.stopPropagation();
    const li = e.currentTarget.closest("li");
    try { await invoke("music_forget", { id: s.id }); keptIds.delete(s.id); keptLoaded = null; li?.remove(); toast(`Deleted ${s.title}`); }
    catch (er) { toast(err(er), true); }
  } }, ic("trash"));
}

const note = (t) => el("p", { class: "mu-note", text: t });
const skeleton = () => el("div", { class: "mu-skel", "aria-label": "Loading" }, Array.from({ length: 3 }, () => el("div", { class: "mu-skel-row" }, el("i", { class: "h" }), el("div", {}, Array.from({ length: 7 }, () => el("i"))))));
function bigPlay(list, from) {
  return el("div", { class: "mu-acts" },
    el("button", { class: "mu-big-play", "aria-label": "Play", title: "Play", onclick: () => playList(list, 0, from) }, ic("play")),
    el("button", { class: "mu-ib mu-big-ib", title: "Shuffle play", "aria-label": "Shuffle play", onclick: () => { if (!P.shuffle) toggleShuffle(); playList(list, Math.floor(Math.random() * list.length), from); } }, ic("shuffle")));
}
function greeting() {
  const h = new Date().getHours();
  return h < 5 ? "Up late?" : h < 12 ? "Good morning" : h < 18 ? "Good afternoon" : "Good evening";
}

/* ---------- the pages ---------- */
let seq = 0;
let chip = "all";

/** Draws a Music page: `m-home`, `m-library[/which]`, `m-album/<id>`, `m-artist/<id>`. */
export async function renderMusic(view, hash) {
  const my = ++seq;
  const [tab, arg] = hash.split("/");
  const page = el("div", { class: "page-top mu-page" });
  view.replaceChildren(page);
  const alive = () => my === seq;
  if (tab === "m-library") return library(page, decodeURIComponent(arg || ""), alive);
  if (tab === "m-album") return albumPage(page, decodeURIComponent(arg || ""), alive);
  if (tab === "m-artist") return artistPage(page, decodeURIComponent(arg || ""), alive);
  if (tab === "m-playlist") return playlistPage(page, decodeURIComponent(arg || ""), alive);
  if (tab === "m-playlists") { page.append(el("header", { class: "mu-hello" }, el("h1", { text: "Playlists" }))); const box = el("div", {}, skeleton()); page.append(box); return playlistsGrid(box, alive); }
  if (tab === "m-downloads") return library(page, "downloaded", alive, true);
  return home(page, alive);
}

async function home(page, alive) {
  const chips = el("div", { class: "mu-chips", role: "group", "aria-label": "Show" });
  const quick = el("div", { class: "mu-quick" });
  const body = el("div", { class: "mu-body" }, skeleton());
  page.append(el("header", { class: "mu-hello" }, el("h1", { text: greeting() })), chips, quick, body);
  const [rows, fy] = await Promise.all([invoke("music_rows").catch(() => []), invoke("music_for_you").catch(() => ({ rows: [], artists: [] })), loadMine(true), loadKept()]);
  if (!alive()) return;
  if (!rows.length && !fy.rows.length) { body.replaceChildren(note("The music charts aren't answering. Try again in a moment.")); return; }

  // Quick picks: what you played lately, else the top of the charts.
  const top = rows.find((r) => r.id === "top");
  const fromRecent = mine.recent.length >= 4;
  const picks = (fromRecent ? mine.recent : top?.items || []).slice(0, 8);
  const myLists = lists().filter((p) => p.songs.length).slice(0, 3);
  const pickFrom = fromRecent ? "Recently played" : "Top songs";
  const liked = mine.liked;
  quick.replaceChildren(
    liked.length ? el("a", { class: "mu-qtile", href: "#m-library/liked" }, el("span", { class: "mu-qart liked-art" }, ic("heart")), el("b", { text: "Liked songs" }), playBadge(() => playList(liked, 0, "Liked songs"))) : null,
    ...myLists.map((p) => el("a", { class: "mu-qtile", href: `#m-playlist/${p.id}` }, cover4(p.songs, "mu-qart"), el("b", { text: p.name }), playBadge(() => playList(p.songs, 0, p.name)))),
    ...picks.slice(0, Math.max(0, 8 - myLists.length - (liked.length ? 1 : 0))).map((s, i) => withMenu(el("div", { class: "mu-qtile", role: "button", tabindex: "0", onclick: () => playList(picks, i, pickFrom), onkeydown: (e) => { if (e.key === "Enter") playList(picks, i, pickFrom); } },
      img(s.art, 120, "mu-qart"), el("b", { text: s.title }), playBadge(() => playList(picks, i, pickFrom))), s)));

  const GENRE_ROWS = rows.filter((r) => r.kind === "songs" && r.id !== "top");
  const CHIPS = [["all", "All"], ...(fy.rows.length ? [["foryou", "For you"]] : []), ["charts", "Charts"], ["albums", "Albums"], ...GENRE_ROWS.map((r) => [r.id, r.title])];
  if (!CHIPS.some(([k]) => k === chip)) chip = "all";
  const draw = () => {
    chips.replaceChildren(...CHIPS.map(([k, label]) => el("button", { class: `mu-chip${chip === k ? " on" : ""}`, "aria-pressed": String(chip === k), onclick: () => { chip = chip === k ? "all" : k; draw(); } }, label)));
    quick.hidden = chip !== "all";
    const out = [];
    const want = (k) => chip === "all" || chip === k;
    if (want("foryou")) {
      for (const r of fy.rows) out.push(row(r.title, r.items.map((s, i) => songCard(s, r.items, i, r.title)), { note: r.note, id: r.id, more: r.artist_id ? el("a", { class: "mu-more", href: `#m-artist/${r.artist_id}`, text: "Artist" }) : el("button", { class: "mu-more", onclick: () => playList(r.items, 0, r.title) }, "Play all") }));
      if (fy.artists.length) out.push(row("Your top artists", fy.artists.map(artistCard), { id: "artists" }));
    }
    if (top && want("charts")) {
      const t10 = top.items.slice(0, 10);
      out.push(row("Top 10 today", t10.map((s, i) => topCard(s, t10, i)), { id: "top10", note: "The most played songs right now", more: el("button", { class: "mu-more", onclick: () => playList(t10, 0, "Top 10") }, "Play all") }));
      const trend = top.items.slice(10);
      if (trend.length) out.push(row("Trending now", trend.map((s, i) => songCard(s, trend, i, "Trending now")), { id: "trending", more: el("button", { class: "mu-more", onclick: () => playList(trend, 0, "Trending now") }, "Play all") }));
    }
    if (chip === "all" && mine.recent.length) out.push(row("Recently played", mine.recent.slice(0, 20).map((s, i, a) => songCard(s, a, i, "Recently played")), { id: "recent", more: el("a", { class: "mu-more", href: "#m-library/recent", text: "Show all" }) }));
    if (want("albums")) for (const r of rows.filter((r) => r.kind === "albums")) out.push(row("New and top albums", r.items.map(albumCard), { id: r.id }));
    let genres = 0;
    for (const r of GENRE_ROWS) {
      if (!want(r.id)) continue;
      // "All" keeps to three genres; the chips have the rest.
      if (chip === "all" && ++genres > 3) break;
      if (chip === r.id) out.push(el("section", { class: "mu-row" }, el("div", { class: "mu-row-head" }, el("h2", { text: r.title })), bigPlay(r.items, r.title), trackList(r.items, { from: r.title })));
      else out.push(row(r.title, r.items.map((s, i) => songCard(s, r.items, i, r.title)), { id: r.id, more: el("button", { class: "mu-more", onclick: () => { chip = r.id; draw(); window.scrollTo({ top: 0, behavior: "smooth" }); } }, "Show all") }));
    }
    body.replaceChildren(...out);
    markPlaying();
  };
  draw();
}

async function playlistsGrid(box, alive) {
  await loadMine(true);
  if (!alive()) return;
  const create = el("button", { class: "mu-card mu-create", onclick: async () => {
    try {
      const p = await invoke("music_playlist_new", { name: `My playlist #${lists().length + 1}`, song: null });
      mine.playlists = [p, ...lists()];
      location.hash = `#m-playlist/${p.id}`;
    } catch (e) { toast(err(e), true); }
  } }, el("span", { class: "mu-cover mu-create-art" }, ic("add")), el("b", { text: "Create playlist" }), el("span", { class: "mu-sub", text: "Name it, then add songs" }));
  box.replaceChildren(el("div", { class: "mu-grid" }, create,
    el("a", { class: "mu-card", href: "#m-library/liked" }, el("span", { class: "mu-cover liked-art mu-cover-ic" }, ic("heart")), el("b", { text: "Liked songs" }), el("span", { class: "mu-sub", text: `${mine.liked.length} songs` })),
    lists().map((p) => el("a", { class: "mu-card", href: `#m-playlist/${p.id}` },
      el("span", { class: "mu-cover" }, cover4(p.songs), p.songs.length ? playBadge(() => playList(p.songs, 0, p.name)) : null),
      el("b", { text: p.name }), el("span", { class: "mu-sub", text: `Playlist · ${p.songs.length} songs` })))));
}

async function playlistPage(page, id, alive) {
  page.append(skeleton());
  await Promise.all([loadMine(true), loadKept()]);
  if (!alive()) return;
  const p = lists().find((x) => x.id === id);
  if (!p) { page.replaceChildren(note("That playlist is gone.")); return; }
  const mins = Math.round(p.songs.reduce((n, s) => n + (s.secs || 0), 0) / 60);
  // The name changes where it is: press it (or the pencil) and type.
  const title = el("h1", { text: p.name, class: "mu-editable", title: "Rename", tabindex: "0" });
  const rename = () => {
    const box = el("input", { type: "text", class: "mu-rename", value: p.name, maxlength: "100", "aria-label": "Playlist name" });
    const done = async (save) => {
      const n = box.value.trim();
      box.replaceWith(title);
      if (!save || !n || n === p.name) return;
      try { await invoke("music_playlist_rename", { id, name: n }); p.name = n; title.textContent = n; refreshLists(); } catch (e) { toast(err(e), true); }
    };
    box.addEventListener("keydown", (e) => { if (e.key === "Enter") done(true); if (e.key === "Escape") done(false); });
    box.addEventListener("blur", () => done(true));
    title.replaceWith(box);
    box.focus();
    box.select();
  };
  title.addEventListener("click", rename);
  title.addEventListener("keydown", (e) => { if (e.key === "Enter") rename(); });
  const del = el("button", { class: "mu-ib mu-big-ib", title: "Delete playlist", "aria-label": "Delete playlist", onclick: async () => {
    if (!(await askSure(`Delete "${p.name}"?`))) return;
    try { await invoke("music_playlist_delete", { id }); mine.playlists = lists().filter((x) => x.id !== id); refreshLists(); location.hash = "#m-playlists"; } catch (e) { toast(err(e), true); }
  } }, ic("trash"));
  const remove = (s) => el("button", { class: "mu-ib", title: "Remove from this playlist", "aria-label": "Remove from this playlist", onclick: async (e) => {
    e.stopPropagation();
    const li = e.currentTarget.closest("li");
    try { await invoke("music_playlist_remove", { id, songId: s.id }); p.songs = p.songs.filter((x) => x.id !== s.id); li?.remove(); toast(`Removed ${s.title}`); refreshLists(); } catch (er) { toast(err(er), true); }
  } }, ic("close"));
  const acts = p.songs.length ? bigPlay(p.songs, p.name) : el("div", { class: "mu-acts" });
  acts.append(el("button", { class: "mu-ib mu-big-ib", title: "Rename", "aria-label": "Rename", onclick: rename }, ic("pen")), del);
  const recBox = el("section", { class: "mu-row mu-rec" });
  page.replaceChildren(
    p.songs[0]?.art ? el("div", { class: "mu-tint", style: `--art:url("${p.songs[0].art}")` }) : null,
    el("header", { class: "mu-hero" }, cover4(p.songs, "mu-hero-art"),
      el("div", {}, el("small", { text: "Playlist" }), title, el("p", { text: [`${p.songs.length} ${p.songs.length === 1 ? "song" : "songs"}`, mins ? `${mins} min` : ""].filter(Boolean).join(" · ") }))),
    acts,
    p.songs.length ? trackList(p.songs, { removable: remove, from: p.name }) : note("Nothing in it yet. Add songs with the + on any song, or from the ones below."),
    recBox);
  markPlaying();
  // Recommended: songs like the ones in it, each with Add.
  const seed = p.songs.length ? p.songs[Math.floor(Math.random() * p.songs.length)] : mine.recent[0] || mine.liked[0];
  if (!seed) return;
  const rec = await invoke("music_radio", { song: seed, skip: p.songs.map((s) => s.id) }).catch(() => []);
  if (!alive() || !rec.length) return;
  const addHere = (s) => el("button", { class: "mu-pill-btn", onclick: async (e) => {
    e.stopPropagation();
    const b = e.currentTarget;
    try { await invoke("music_playlist_add", { id, song: s }); p.songs.push(s); b.textContent = "Added"; b.disabled = true; refreshLists(); } catch (er) { toast(err(er), true); }
  } }, "Add");
  recBox.replaceChildren(el("div", { class: "mu-row-head" }, el("div", {}, el("h2", { text: "Recommended" }), el("p", { class: "mu-row-note", text: p.songs.length ? `Based on what's in ${p.name}` : "Based on what you listen to" }))),
    trackList(rec.slice(0, 10), { removable: addHere, numbered: false, from: "Recommended" }));
  markPlaying();
}

/** A themed "are you sure" (api.js ask), not the browser's own. */
function askSure(q) {
  return ask(q, "This can't be undone.", [], "Delete", "Cancel");
}

async function library(page, which, alive, alone = false) {
  // Playlists and Downloaded have their own tabs now; old links go there.
  if (which === "playlists") { location.replace("#m-playlists"); return; }
  if (which === "downloaded" && !alone) { location.replace("#m-downloads"); return; }
  const tabs = [["liked", "Liked songs"], ["recent", "Recently played"]];
  if (!alone && !tabs.some(([k]) => k === which)) which = ["liked", "recent"].includes(getPref("music-lib")) ? getPref("music-lib") : "liked";
  if (!alone) setPref("music-lib", which);
  const box = el("div", {}, skeleton());
  if (!alone) page.append(el("div", { class: "mu-chips mu-chips-top" }, tabs.map(([k, label]) => el("a", { class: `mu-chip${k === which ? " on" : ""}`, href: `#m-library/${k}` }, label))));
  page.append(box);
  const [list] = await Promise.all([which === "downloaded" ? loadKept(true) : loadMine(true).then((m) => (which === "liked" ? m.liked : m.recent)), loadKept(), loadMine()]);
  if (!alive()) return;
  const title = which === "downloaded" ? "Downloaded" : tabs.find(([k]) => k === which)[1];
  const cover = el("span", { class: `mu-hero-art ${which}-art` }, ic(which === "liked" ? "heart" : which === "downloaded" ? "down" : "clock"));
  const empty = { liked: "Songs you like show up here. Press the heart on any song.", downloaded: "Nothing downloaded yet. Choose Download in any song's ⋯ menu to keep it in Music\\Chillarr; it then plays from this PC with no waiting.", recent: "Songs you play show up here." }[which];
  box.replaceChildren(
    el("header", { class: "mu-hero" }, cover, el("div", {}, el("small", { text: "Playlist" }), el("h1", { text: title }), el("p", { text: `${list.length} ${list.length === 1 ? "song" : "songs"}` }))),
    list.length ? bigPlay(list, title) : null,
    list.length ? trackList(list, { removable: which === "downloaded", from: title }) : note(empty));
  markPlaying();
}

async function albumPage(page, id, alive) {
  page.append(skeleton());
  try {
    const [{ album, songs }] = await Promise.all([invoke("music_album", { id }), loadKept(), loadMine()]);
    if (!alive()) return;
    const mins = Math.round(songs.reduce((n, s) => n + (s.secs || 0), 0) / 60);
    const by = album.artist_id ? el("a", { href: `#m-artist/${album.artist_id}`, text: album.artist }) : el("b", { text: album.artist });
    page.replaceChildren(
      el("div", { class: "mu-tint", style: `--art:url("${album.art}")` }),
      el("header", { class: "mu-hero" }, el("img", { class: "mu-hero-art", src: album.art, alt: "" }),
        el("div", {}, el("small", { text: "Album" }), el("h1", { text: album.title }),
          el("p", {}, by, [album.year, `${songs.length} songs`, mins ? `${mins} min` : ""].filter(Boolean).map((t) => ` · ${t}`).join("")))),
      bigPlay(songs, album.title),
      trackList(songs, { album: false, from: album.title }));
    markPlaying();
  } catch (e) { if (alive()) page.replaceChildren(note(err(e))); }
}

async function artistPage(page, id, alive) {
  page.append(skeleton());
  try {
    const [{ artist, songs, albums }] = await Promise.all([invoke("music_artist", { id }), loadKept(), loadMine()]);
    if (!alive()) return;
    let all = false;
    const popular = el("div");
    const drawPopular = () => {
      popular.replaceChildren(trackList(songs.slice(0, all ? 20 : 5), { from: artist.name }),
        songs.length > 5 ? el("button", { class: "mu-more mu-showmore", onclick: () => { all = !all; drawPopular(); } }, all ? "Show less" : "See more") : null);
      markPlaying();
    };
    drawPopular();
    const singles = albums.filter((a) => a.count <= 3);
    const lps = albums.filter((a) => a.count > 3);
    page.replaceChildren(
      el("header", { class: "mu-artist-hero", style: `--art:url("${artist.art}")` },
        el("div", {}, el("small", { text: artist.genre || "Artist" }), el("h1", { text: artist.name }))),
      bigPlay(songs, artist.name),
      el("section", { class: "mu-row" }, el("div", { class: "mu-row-head" }, el("h2", { text: "Popular" })), popular),
      lps.length ? row("Albums", lps.map(albumCard)) : null,
      singles.length ? row("Singles and EPs", singles.map(albumCard)) : null);
  } catch (e) { if (alive()) page.replaceChildren(note(err(e))); }
}

/** Search results while in Music, with filters. */
export async function renderMusicSearch(view, term) {
  const my = ++seq;
  const chips = el("div", { class: "mu-chips" });
  const body = el("div", {}, skeleton());
  view.replaceChildren(el("div", { class: "page-top mu-page" }, chips, body));
  try {
    const [r] = await Promise.all([invoke("music_search", { term }), loadKept(), loadMine()]);
    if (my !== seq) return;
    const { songs, albums, artists = [] } = r;
    // YouTube's own results take a few seconds: they fill in when they come.
    let yt = null;
    const ytP = invoke("music_youtube", { term }).then((l) => { yt = l; if (my === seq) draw(); }).catch((e) => { yt = err(e); if (my === seq) draw(); });
    void ytP;
    /** Remixes, covers, live takes and edits, from YouTube itself. `n`: how many (0: all). */
    const ytBox = (n) => {
      const head = el("div", { class: "mu-row-head" }, el("div", {}, el("h2", {}, ic("yt", "mu-yt-ic"), "From YouTube"), el("p", { class: "mu-row-note", text: "Remixes, covers, live versions, sped up and more" })),
        n && Array.isArray(yt) && yt.length > n ? el("button", { class: "mu-more", onclick: () => { f = "youtube"; draw(); } }, "Show all") : null);
      if (yt === null) return el("section", { class: "mu-row mu-yt" }, head, el("div", { class: "mu-table" }, Array.from({ length: Math.min(n || 6, 4) }, () => el("div", { class: "mu-skel-track" }))));
      if (typeof yt === "string") return el("section", { class: "mu-row mu-yt" }, head, note(yt));
      if (!yt.length) return el("section", { class: "mu-row mu-yt" }, head, note("Nothing on YouTube for that."));
      const list = n ? yt.slice(0, n) : yt;
      return el("section", { class: "mu-row mu-yt" }, head, n ? null : bigPlay(yt, "YouTube"), trackList(list, { numbered: false, from: "YouTube" }));
    };
    let f = !songs.length && !albums.length ? "youtube" : "all";
    const draw = () => {
      chips.replaceChildren(...[["all", "All"], ["songs", "Songs"], ["youtube", "YouTube"], ["artists", "Artists"], ["albums", "Albums"]].map(([k, l]) => el("button", { class: `mu-chip${f === k ? " on" : ""}`, onclick: () => { f = k; draw(); } }, k === "youtube" ? [ic("yt"), l] : l)));
      const out = [];
      if (f === "all") {
        // The top result: the artist when the search is their name, else the first song.
        const a0 = artists[0];
        const isArtist = a0 && a0.name.toLowerCase().includes(term.toLowerCase().trim()) ;
        const best = isArtist
          ? el("a", { class: "mu-best", href: `#m-artist/${a0.id}` }, img(a0.art, 240, "round"), el("h3", { text: a0.name }), el("span", { class: "mu-pill", text: "Artist" }), playBadge(() => playArtist(a0.id, a0.name)))
          : songs[0] ? el("div", { class: "mu-best", role: "button", tabindex: "0", onclick: () => playList(songs, 0, "Search") }, img(songs[0].art, 240), el("h3", { text: songs[0].title }), el("span", {}, el("span", { class: "mu-pill", text: "Song" }), ` ${songs[0].artist}`), playBadge(() => playList(songs, 0, "Search"))) : null;
        out.push(el("div", { class: "mu-search-top" },
          el("section", {}, el("h2", { text: "Top result" }), best),
          el("section", {}, el("h2", { text: "Songs" }), trackList(songs.slice(0, 4), { album: false, numbered: false, from: "Search" }))));
        out.push(ytBox(6));
        if (artists.length) out.push(row("Artists", artists.map(artistCard)));
        if (albums.length) out.push(row("Albums", albums.map(albumCard)));
      } else if (f === "youtube") out.push(ytBox(0));
      else if (f === "songs") out.push(bigPlay(songs, "Search"), trackList(songs, { numbered: false, from: "Search" }));
      else if (f === "artists") out.push(artists.length ? el("div", { class: "mu-grid" }, artists.map(artistCard)) : note("No artists."));
      else out.push(albums.length ? el("div", { class: "mu-grid" }, albums.map(albumCard)) : note("No albums."));
      body.replaceChildren(...out);
      markPlaying();
    };
    draw();
  } catch (e) { if (my === seq) body.replaceChildren(note(err(e))); }
}

export const musicIcon = I.note;
