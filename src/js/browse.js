// Home, Movies, Series, Anime and My downloads: a hero, genre chips and rows
// of posters; hover previews; Top 10; and "Explore all" pages that keep loading.
import { isHidden, markHidden } from "./hidden.js";
import { $, ask, canDelete, choose, el, expandRow, genresAZ, getPref, icon, ICON, invoke, listen, metaBits, personKey, picProps, store, toast } from "./api.js";
import { openDetail, playShowNow } from "./detail.js";
import { canWatchNow, confirmRemove, isWatchNow, movieLike, play, removeWatchNow, watchNow } from "./player.js";
import { completedActions, confirmRedownload, listable, markWatched, onMyList, removeCompleted, removeContinue, resumable, setMyList } from "./mylist.js";
import { goBack, remember, restore } from "./place.js";
import { inlinePicker, styleNow } from "./look.js";
import { tabbed } from "./read.js";
import { FILTER_KIND, inlineFilters } from "./filter.js";
import { adultOf, adultShows } from "./filterpanel.js";
import { selectable, stopSelecting } from "./select.js";

const WIDE = new Set(["continue", "downloading"]);
let refreshTimer;
let heroTimer;
let emptyTimer;
let renderSeq = 0;

/** Stops this page's timers (called when leaving for Settings). */
/** The current page's render number, for pages built in other files. */
export const currentSeq = () => renderSeq;

export function stopBrowse() {
  renderSeq++;
  stopSelecting();
  exploreObserver?.disconnect();
  exploreObserver = null;
  lazyObserver?.disconnect();
  lazyObserver = null;
  clearTimeout(emptyTimer);
  clearInterval(refreshTimer);
  clearInterval(heroTimer);
  heroShow = null;
  for (const t of [...fitsWatched]) unwatchFits(t);
  hidePreview(true);
  // A kept tab going out of view: its rows' sideways scroll is noted (taking
  // a box out of the page loses it), and its entrances may play again later.
  if (onScreen) { for (const t of tracksOf(onScreen)) t._left = t.scrollLeft; onScreen = null; }
  $("#view").classList.remove("kept");
}

/** A title with no picture: a designed card with its name, tinted per title so a row of them isn't one grey block. */
function noArt(t) {
  let hue = 0;
  for (const c of t.title) hue = (hue * 31 + c.codePointAt(0)) % 360;
  const kind = { movie: "Movie", series: "Series", anime: "Anime", episode: "Episode" }[t.kind] || "";
  return el("div", { class: "noart", style: `--h:${hue}` },
    icon("M4 5h16v14H4zM8 5v14M16 5v14M4 9.5h4M4 14.5h4M16 9.5h4M16 14.5h4"),
    el("strong", { text: t.title }),
    el("small", { text: t.year ? String(t.year) : kind }));
}

function art(t, wide) {
  // Jellyfin's backdrops are asked for at the hero's 1600px; a card is about 300.
  const src = wide ? (t.backdrop || t.poster || "").replace("fillWidth=1600", "fillWidth=640") : t.poster || t.backdrop;
  const box = el("div", { class: "art" });
  if (src) {
    // The size the card is drawn at (styles.css: 112px / 228px on a phone), not the biggest there is.
    // (A backdrop standing in for a poster is cropped to a sliver of it: kept as it is.)
    const poster = src === t.poster;
    const fit = wide ? picProps(src, [300, 480, 640], poster ? 1.5 : 0.5625, "(max-width: 600px) 228px, 300px")
      : poster ? picProps(src, [185, 342, 500], 1.5, "(max-width: 600px) 112px, 200px") : { src };
    const img = el("img", { ...fit, alt: "", loading: "lazy", decoding: "async" });
    img.addEventListener("load", () => img.classList.add("loaded"), { once: true });
    img.addEventListener("error", () => img.replaceWith(noArt(t)));
    box.append(img);
  } else {
    box.append(noArt(t));
  }
  // A Watch now title says where its download is under the title instead (see tile()).
  if (t.status && !isWatchNow(t)) box.append(el("div", { class: "over", text: t.status }));
  // The rating on the cover: only the Midnight style shows it (styles.css).
  if (t.rating && !wide) box.append(el("span", { class: "rc-score", text: `★ ${(+t.rating).toFixed(1)}` }));
  if (t.progress != null && (t.jf_id || t.status)) {
    box.append(el("div", { class: "bar" }, el("span", { style: `width:${Math.round(t.progress * 100)}%` })));
  }
  markWatched(box, t);
  return box;
}

function open(t) {
  if (isWatchNow(t)) watchNow(t);
  else if (t.kind === "episode" && t.jf_id) play(t);
  else openDetail(t);
}

/** Play / Resume on a hover card: plays at once, never the panel (unless the episode you're at isn't here). */
async function cardPlay(t) {
  if (isWatchNow(t)) return watchNow(t);
  if (movieLike(t) || t.kind === "episode") return play(t);
  if (!(await playShowNow(t))) openDetail(t);
}

/** Download on a hover card: a film starts downloading at once (the quality last picked); a show opens its picker, since it needs to know which episodes. */
async function cardDownload(t) {
  if (!movieLike(t)) return openDetail(t, { get: true });
  if (!(await confirmRedownload(t))) return;
  const quality = getPref("chillarr-quality") || "best";
  try {
    toast(`${await invoke("media_get", { tile: t, quality })} You'll get a notification when it's ready.`);
    store.emit("added");
  } catch (e) { toast(String(e), true); }
}

/** Watch now from a card or the hero: a movie plays, a show plays the episode you're at. */
// A show opens at its episodes so you pick which one; a movie just plays.
const watchFrom = (t) => (movieLike(t) ? watchNow(t) : openDetail(t, { episodes: true }));

export function tile(t, wide = false, rank = 0) {
  const cap = el("div", { class: "cap" }, el("strong", { text: t.title }), t.subtitle ? ` · ${t.subtitle}` : t.year ? ` · ${t.year}` : "");
  const cls = ["tile", wide ? "wide" : "", rank ? "top10" : ""].filter(Boolean).join(" ");
  // A Top 10 card has no caption under it; Classic lists the title and genre beside the cover instead (styles.css).
  const rkCap = rank ? el("div", { class: "rk-cap" }, el("strong", { text: t.title }), el("small", { text: [t.year, genresAZ(t.genres || [], 1)[0]].filter(Boolean).join(" · ") })) : null;
  const b = el("button", { class: cls, "aria-label": t.title, dataset: { key: t.key } }, rank ? el("span", { class: "rank", "aria-hidden": "true", text: String(rank) }) : null, art(t, wide), rank ? rkCap : cap);
  b._tsig = `${rank}|${JSON.stringify(t)}`; // what it shows; a kept tab redraws a card only when this changes
  markHidden(b, t);
  if (isWatchNow(t) && t.status && !rank) b.append(el("div", { class: "cap wn-state", text: t.status }));
  b.addEventListener("click", () => { hidePreview(true); open(t); });
  b.addEventListener("mouseenter", () => schedulePreview(b, t));
  b.addEventListener("mouseleave", () => leaveSoon());
  return b;
}

/* ---------- Hover preview card ---------- */
// Hover-card trailers: the connection to YouTube is made ahead of the first
// one, only where there are hover cards (a mouse). A phone on a weak line
// doesn't spend its first second on two handshakes it never uses.
if (matchMedia("(hover: hover) and (pointer: fine)").matches && !document.documentElement.classList.contains("tv")) {
  for (const href of ["https://www.youtube-nocookie.com", "https://i.ytimg.com"]) document.head.append(el("link", { rel: "preconnect", href }));
}
// A poster opens like a door: the poster stays under the pointer, a little
// larger, and the details swing out to the side with room to read, with the
// trailer playing above them. Wide cards (Continue watching) already are the
// picture, so they just grow. Its picture is fetched and decoded while the
// pointer is still settling, and the trailer takes over once it's clean.
let preview = null;
let previewTimer;
let leaveTimer;
let trailerTimer;
let soundOn = false;
try { soundOn = localStorage.getItem("chillarr-pv-sound") === "1"; } catch (e) { /* storage blocked */ }
const OPEN_DELAY = 420;
const SWITCH_DELAY = 140;
// YouTube loads in its own process, so it can start with the card.
const TRAILER_DELAY = 0;
const TRAILER_CLEAN = 3.8;
// YouTube draws its own title bar, a pause icon and "More videos" over the
// first ~3.7s of playback, often but not always, whatever the embed settings
// (filmed frame by frame in Edge and in the app). So the video starts early
// and stays hidden until it has played past that: what you see starts at
// about 5s in, after the studio logos most trailers open with.
const TRAILER_START = 1;

/** The card's picture: a TMDB backdrop at 780px is plenty for a ~400px card. */
function cardPic(t) {
  const pic = t.backdrop || t.poster;
  return pic ? pic.replace("/w1280/", "/w780/").replace("/original/", "/w780/").replace("fillWidth=1600", "fillWidth=780") : "";
}

/** Loads `src` into `img`; resolves once it can be drawn. decode() is Chrome 64: older ones wait for load. */
function decoded(img, src) {
  if (img.decode) { img.src = src; return img.decode(); }
  const p = new Promise((done, fail) => { img.onload = done; img.onerror = fail; });
  img.src = src;
  return p;
}

const warmed = new Map();
/** Starts fetching and decoding the card's picture before the card opens. */
function warm(t) {
  const src = cardPic(t);
  if (!src || warmed.has(src)) return;
  const img = new Image();
  img.decoding = "async";
  const ready = decoded(img, src);
  warmed.set(src, ready.catch(() => {}));
  if (warmed.size > 200) warmed.delete(warmed.keys().next().value);
}

/** The trailer's YouTube id, asked for once per title while the card opens. */
const trailerIds = new Map();
function trailerId(t) {
  if (t.trailer) return Promise.resolve(t.trailer);
  if (!trailerIds.has(t.key)) trailerIds.set(t.key, invoke("media_trailer", { tile: t }).catch(() => null));
  return trailerIds.get(t.key);
}

const REMOVE = "M18 6 6 18M6 6l12 12";

/* The card's fixed size: the shape never depends on what's loaded, so it opens in one go. */
const INFO_W = 356;
const CARD_H = 418;
const POSTER_W = Math.round(CARD_H * 2 / 3);

function previewCard(t, door) {
  const actions = el("div", { class: "pv-actions" });
  const act = (fn) => (e) => { e.stopPropagation(); hidePreview(true); fn(); };
  const round = (cls, label, path, onclick) => el("button", { class: `pv-btn ${cls}`, "aria-label": label, title: label, onclick: act(onclick) }, icon(path));
  const pill = (label, path, onclick) => el("button", { class: "pv-pill", onclick: act(onclick) }, icon(path), label);
  // A completed title still in the library plays like any other, with Remove from Completed.
  if (t.completed && !t.jf_id) {
    const [first, ...rest] = completedActions(t, openDetail);
    actions.append(pill(first.label, first.icon, first.run), ...rest.map((a) => round("", a.label, a.icon, a.run)));
  } else if (isWatchNow(t)) {
    actions.append(pill(t.resume_seconds ? "Resume" : "Play", ICON.play, () => watchNow(t)), round("", "Remove", REMOVE, () => removeWatchNow(t)));
  } else {
    if (t.jf_id) actions.append(pill(t.resume_seconds ? "Resume" : "Play", ICON.play, () => cardPlay(t)));
    else if (!t.status && canWatchNow(t)) actions.append(pill("Watch now", ICON.play, () => watchFrom(t)), round("", "Download", ICON.download, () => cardDownload(t)));
    else if (!t.status) actions.append(pill("Download", ICON.download, () => cardDownload(t)));
    if (listable(t)) actions.append(myListRound(t));
    if (resumable(t)) actions.append(round("", "Remove from Continue watching", REMOVE, () => removeContinue(t)));
    if (t.completed) actions.append(round("", "Remove from Completed", REMOVE, () => removeCompleted(t)));
  }
  actions.append(el("span", { style: "flex:1" }));
  actions.append(round("", "More info", "m6 9 6 6 6-6", () => openDetail(t)));
  const src = cardPic(t);
  const pic = src ? el("img", { class: "pv-img", src, alt: "", decoding: "async" }) : el("div", { class: "pv-img noart" });
  if (src) {
    if (pic.complete && pic.naturalWidth) pic.classList.add("loaded");
    else pic.addEventListener("load", () => pic.classList.add("loaded"), { once: true });
  }
  const meta = [];
  const kind = { movie: "Movie", series: "Series", anime: "Anime" }[t.kind];
  if (t.rating) meta.push(el("span", { class: "rating", text: `★ ${(+t.rating).toFixed(1)}` }));
  if (t.year) meta.push(el("span", { text: String(t.year) }));
  if (kind) meta.push(el("span", { class: "kind", text: kind }));
  if (t.completed) meta.push(el("span", { class: "owned", text: t.mark ? `${t.completed} · ${t.mark}` : t.completed }));
  else if (isWatchNow(t)) meta.push(el("span", { class: "owned", text: t.status || "Watch now" }));
  else if (t.jf_id) meta.push(el("span", { class: "owned", text: "In your library" }));
  else if (t.status) meta.push(el("span", { class: "owned", text: t.status }));
  const info = el("div", { class: "pv-info" },
    el("div", { class: "pv-art" }, pic),
    el("div", { class: "pv-body" },
      el("strong", { class: "pv-title", text: t.title }),
      meta.length ? el("div", { class: "pv-meta" }, meta) : null,
      t.overview ? el("p", { class: "pv-overview", text: t.overview }) : null,
      t.genres?.length ? el("div", { class: "pv-genres" }, genresAZ(t.genres, 3).map((g) => el("span", { text: g }))) : null,
      actions));
  const poster = door ? el("div", { class: "pv-poster" }, el("img", { src: t.poster, alt: "", decoding: "async" }),
    t.progress != null && (t.jf_id || t.status) ? el("div", { class: "pv-progress" }, el("span", { style: `width:${Math.round(t.progress * 100)}%` })) : null) : null;
  return el("div", { class: door ? "preview door" : "preview", role: "dialog", "aria-label": t.title }, poster, info);
}

/** My List from the card: flips in place, the card stays open. */
function myListRound(t) {
  const b = el("button", { class: "pv-btn", "aria-label": "My List", title: "My List" }, icon(ICON.plus));
  const draw = (on) => { b.replaceChildren(icon(on ? ICON.check : ICON.plus)); b.title = on ? "On My List" : "My List"; b.dataset.on = on ? "1" : ""; };
  onMyList(t).then(draw).catch(() => {});
  b.addEventListener("click", async (e) => {
    e.stopPropagation();
    b.disabled = true;
    try {
      const on = await setMyList(t, !b.dataset.on);
      draw(on);
      toast(on ? `Added ${t.title} to My List.` : `Took ${t.title} off My List.`);
    } catch (err) { toast(String(err), true); }
    b.disabled = false;
  });
  return b;
}

function schedulePreview(anchor, t) {
  // No hover cards by touch, or on a TV (LG's pointer remote hovers, but the cards are for a mouse).
  if (matchMedia("(hover: none)").matches || document.documentElement.classList.contains("tv")) return;
  clearTimeout(previewTimer);
  clearTimeout(leaveTimer);
  warm(t);
  // Moving from one open card to the next poster: follow quickly.
  previewTimer = setTimeout(() => showPreview(anchor, t), preview ? SWITCH_DELAY : OPEN_DELAY);
}

async function showPreview(anchor, t) {
  const art = anchor.querySelector(".art");
  if (!art || !anchor.isConnected) return;
  // A picture that is nearly ready is worth a short wait: the card then opens
  // whole instead of flashing in empty.
  const ready = warmed.get(cardPic(t));
  if (ready) await Promise.race([ready, new Promise((r) => setTimeout(r, 160))]);
  if (!anchor.matches(":hover")) return;
  hidePreview();
  trailerId(t); // look it up while the card grows
  const r = art.getBoundingClientRect();
  // A portrait poster with room to open beside it; otherwise the card alone.
  const door = !!t.poster && r.height > r.width && innerWidth >= POSTER_W + INFO_W + 40;
  const w = door ? POSTER_W + INFO_W : Math.min(Math.max(r.width * 1.5, 340), INFO_W + 40);
  const h = door ? CARD_H : w * 9 / 16 + (CARD_H - INFO_W * 9 / 16);
  const cx = r.left + r.width / 2;
  // The poster stays under the pointer. It opens to the right unless that runs off the screen.
  const flip = door && cx + POSTER_W / 2 + INFO_W > innerWidth - 12;
  let left = door ? (flip ? cx + POSTER_W / 2 - w : cx - POSTER_W / 2) : cx - w / 2;
  left = Math.min(Math.max(12, left), innerWidth - w - 12);
  const top = Math.min(Math.max(76, r.top + r.height / 2 - h / 2), innerHeight - h - 12);
  const card = previewCard(t, door);
  if (flip) card.classList.add("flip");
  card.style.cssText = `left:${left}px;top:${top}px;width:${w}px;height:${h}px;--pw:${POSTER_W}px`;
  card.addEventListener("mouseenter", () => clearTimeout(leaveTimer));
  card.addEventListener("mouseleave", () => leaveSoon());
  card.addEventListener("click", () => { hidePreview(true); open(t); });
  document.body.append(card);
  const still = matchMedia("(prefers-reduced-motion: reduce)").matches;
  if (door && !still) {
    // The part over the poster starts at the poster's exact size and place, and the rest is uncovered from it.
    const px = flip ? w - POSTER_W : 0;
    const s = Math.max(0.5, r.width / POSTER_W);
    const dx = cx - (left + px + POSTER_W / 2);
    const dy = r.top + r.height / 2 - (top + h / 2);
    card.style.transformOrigin = `${px + POSTER_W / 2}px ${h / 2}px`;
    const shut = flip ? `inset(0 0 0 ${w - POSTER_W}px round 10px)` : `inset(0 ${w - POSTER_W}px 0 0 round 10px)`;
    card.animate(
      [{ transform: `translate(${dx}px, ${dy}px) scale(${s})`, clipPath: shut }, { transform: "none", clipPath: "inset(0 0 0 0 round 10px)" }],
      { duration: 340, easing: "cubic-bezier(0.16, 1, 0.3, 1)" },
    );
  } else if (!still) {
    const dx = cx - (left + w / 2);
    const dy = r.top + r.height / 2 - (top + h / 2);
    card.animate(
      [{ opacity: 0, transform: `translate(${dx}px, ${dy}px) scale(${Math.max(0.5, r.width / w)})` }, { opacity: 1, transform: "none" }],
      { duration: 300, easing: "cubic-bezier(0.16, 1, 0.3, 1)" },
    );
  }
  preview = card;
  clearTimeout(trailerTimer);
  trailerTimer = setTimeout(() => startTrailer(card, t), TRAILER_DELAY);
}

function leaveSoon() {
  clearTimeout(previewTimer);
  clearTimeout(leaveTimer);
  leaveTimer = setTimeout(() => hidePreview(), 160);
}

function hidePreview(now = false) {
  clearTimeout(previewTimer);
  clearTimeout(trailerTimer);
  if (!preview) return;
  if (listChanged) { listChanged = false; setTimeout(() => window.dispatchEvent(new Event("chillarr:refresh")), 200); }
  const p = preview;
  preview = null;
  p.style.pointerEvents = "none";
  p.querySelector(".pv-video")?.remove();
  if (now) { p.remove(); return; }
  // onfinish, not .finished (Chrome 84).
  const out = p.animate([{ opacity: 1, transform: "none" }, { opacity: 0, transform: "scale(0.96)" }], { duration: 160, easing: "ease-in", fill: "forwards" });
  out.onfinish = out.oncancel = () => p.remove();
}

/* Trailer snippet: a muted, chrome-less YouTube player that stays invisible
   until YouTube reports it is actually playing, so a slow load or a video that
   can't be embedded just leaves the picture showing. */
async function startTrailer(card, t) {
  if (preview !== card) return;
  const id = await trailerId(t);
  if (!id || preview !== card) return;
  const art = card.querySelector(".pv-art");
  const params = new URLSearchParams({
    autoplay: "1", mute: "1", controls: "0", disablekb: "1", fs: "0", rel: "0",
    modestbranding: "1", playsinline: "1", iv_load_policy: "3", start: String(TRAILER_START),
    enablejsapi: "1", origin: location.origin,
  });
  const frame = el("iframe", {
    class: "pv-video",
    src: `https://www.youtube-nocookie.com/embed/${encodeURIComponent(id)}?${params}`,
    allow: "autoplay; encrypted-media",
    tabindex: "-1",
    title: `${t.title} trailer`,
  });
  frame.addEventListener("load", () => send(frame, { event: "listening", id: 1, channel: "widget" }), { once: true });
  const sound = el("button", { class: "pv-sound", "aria-label": "Sound", title: "Sound", onclick: (e) => {
    e.stopPropagation();
    soundOn = !soundOn;
    try { localStorage.setItem("chillarr-pv-sound", soundOn ? "1" : "0"); } catch (err) { /* storage blocked */ }
    applySound(frame, sound);
  } });
  applySound(null, sound);
  art.append(frame, sound);
}

function send(frame, msg) {
  frame.contentWindow?.postMessage(JSON.stringify(msg), "https://www.youtube-nocookie.com");
}

function command(frame, func, args = []) {
  send(frame, { event: "command", func, args, id: 1, channel: "widget" });
}

const SPEAKER = "M11 5 6 9H3v6h3l5 4z";
function applySound(frame, btn) {
  btn.replaceChildren(icon(soundOn ? `${SPEAKER}M15.5 8.5a5 5 0 0 1 0 7M18.5 5.5a9 9 0 0 1 0 13` : `${SPEAKER}M22 9l-6 6M16 9l6 6`));
  btn.classList.toggle("on", soundOn);
  if (frame) command(frame, soundOn ? "unMute" : "mute");
}

window.addEventListener("message", (e) => {
  if (e.origin !== "https://www.youtube-nocookie.com" || !preview) return;
  const frame = preview.querySelector(".pv-video");
  if (!frame || e.source !== frame.contentWindow) return;
  let data;
  try { data = typeof e.data === "string" ? JSON.parse(e.data) : e.data; } catch (err) { return; }
  const state = data?.event === "onStateChange" ? data.info : data?.info?.playerState;
  if (state != null) frame.dataset.state = state;
  if (typeof data?.info?.currentTime === "number") frame.dataset.time = data.info.currentTime;
  const art = preview.querySelector(".pv-art");
  // Shown only once YouTube's start-up overlay is gone (see TRAILER_START).
  const clean = +frame.dataset.state === 1 && +(frame.dataset.time || 0) >= TRAILER_START + TRAILER_CLEAN;
  if (clean && !art.classList.contains("playing")) {
    art.classList.add("playing");
    if (soundOn) command(frame, "unMute");
  } else if (state === 0) {
    art.classList.remove("playing"); // ended: back to the picture
  }
});
window.addEventListener("scroll", () => hidePreview(true), { passive: true });

/* ---------- Rows ---------- */
document.addEventListener("click", (e) => {
  if (e.target.closest(".explore")) {
    try { sessionStorage.setItem("chillarr-from", location.hash || "#home"); } catch (err) { /* storage blocked */ }
  }
});

function exploreHref(r) {
  return `#explore/${encodeURIComponent(r.explore)}/${encodeURIComponent(r.title)}`;
}

/** A row whose posters all fit doesn't scroll and shows no arrows. */
const fitsWatcher = new ResizeObserver((entries) => {
  for (const e of entries) {
    const t = e.target;
    if (!t.isConnected) { unwatchFits(t); continue; }
    t.parentElement.classList.toggle("fits", t.scrollWidth - t.clientWidth < 48);
  }
});
// What it watches, so a page going away lets go of all of its rows.
const fitsWatched = new Set();
function watchFits(track) { fitsWatched.add(track); fitsWatcher.observe(track); }
function unwatchFits(track) { fitsWatched.delete(track); fitsWatcher.unobserve(track); }

function rowEl(r, i = 0) {
  const wide = WIDE.has(r.id);
  const top10 = r.style === "top10";
  const track = el("div", { class: top10 ? "track top10-track" : "track" }, r.tiles.map((t, j) => { const b = tile(t, wide, top10 ? j + 1 : 0); b.style.setProperty("--j", Math.min(j, 10)); return b; }));
  if (r.style === "lazy") track.append(...Array.from({ length: 8 }, () => el("div", { class: "skel skel-tile" })));
  track.addEventListener("scroll", () => hidePreview(true), { passive: true });
  const step = () => track.clientWidth * 0.85;
  const prev = el("button", { class: "arrow prev", "aria-label": "Scroll left", onclick: () => track.scrollBy({ left: -step(), behavior: "smooth" }) }, icon(ICON.left));
  const next = el("button", { class: "arrow next", "aria-label": "Scroll right", onclick: () => track.scrollBy({ left: step(), behavior: "smooth" }) }, icon(ICON.right));
  watchFits(track);
  const head = el("div", { class: "row-head" }, el("h2", { text: r.title }),
    r.explore ? el("a", { class: "explore", href: exploreHref(r) }, "Explore all", icon(ICON.right)) : el("span", { class: "count", text: String(r.tiles.length) }));
  return el("section", { class: "row", dataset: { row: r.id }, id: `row-${r.id}`, style: `--i:${Math.min(i, 6)}` }, head, el("div", { class: "track-wrap" }, prev, track, next));
}

/** Filters (Home's opens them as a page; Movies, Series and Anime have them on
 *  the page itself, filter.js inlineFilters), then genre chips that jump to their row. */
function chipBar(rows, tab) {
  const genres = rows.filter((r) => r.id.startsWith("genre-"));
  const filters = FILTER_KIND[tab] ? null : el("button", { class: "chip filter-chip", onclick: () => {
    try { sessionStorage.setItem("chillarr-from", location.hash || "#home"); } catch (e) { /* storage blocked */ }
    location.hash = `#filter/${KIND_FOR_TAB[tab] || "movie"}/{}`;
  } }, icon("M3 5h18M6 12h12M10 19h4"), "Filters");
  if (!filters && genres.length < 3) return el("nav", { class: "chips genre-chips empty", "aria-label": "Genres" });
  return el("nav", { class: "chips genre-chips", "aria-label": "Genres" }, filters, genres.length >= 3 ? genres.map((r) =>
    el("button", { class: "chip", onclick: () => { const t = document.getElementById(`row-${r.id}`); if (t) window.scrollTo({ top: t.getBoundingClientRect().top + scrollY - 90, behavior: "smooth" }); } }, r.title)) : null);
}

/* ---------- Hero ---------- */
function pickHeroes(rows) {
  const order = ["continue", "top10", "trending", "because-0", "anime-trending", "popular-series", "top-series", "new", "latest", "library-movies"];
  const out = [];
  for (const id of order) {
    const r = rows.find((x) => x.id === id);
    for (const t of r?.tiles || []) {
      if (t.backdrop && out.length < 5 && !out.some((o) => o.t.key === t.key)) {
        const kicker = id === "continue" ? "Continue watching" : id === "top10" ? `#${r.tiles.indexOf(t) + 1} this week` : id === "latest" || id.startsWith("library") ? "In your library" : id === "because-0" ? r.title : "Recommended for you";
        out.push({ t, kicker });
        if (out.length >= (id === "continue" ? 1 : 5) || (id === "top10" && out.length >= 3)) break;
      }
    }
  }
  return out.slice(0, 5);
}

function heroCopy({ t, kicker }) {
  const actions = el("div", { class: "actions" });
  if (t.jf_id) {
    actions.append(el("button", { class: "btn play", onclick: () => (t.kind === "series" ? openDetail(t) : play(t)) }, icon(ICON.play), t.resume_seconds ? "Resume" : "Play"));
  } else if (!t.status && canWatchNow(t)) {
    actions.append(el("button", { class: "btn play", onclick: () => watchFrom(t) }, icon(ICON.play), "Watch now"));
    actions.append(el("button", { class: "btn soft", onclick: () => openDetail(t, { get: true }) }, icon(ICON.download), "Download"));
  } else if (!t.status) {
    actions.append(el("button", { class: "btn play", onclick: () => openDetail(t, { get: true }) }, icon(ICON.download), "Download"));
  }
  actions.append(el("button", { class: "btn soft", onclick: () => openDetail(t) }, icon(ICON.info), "More info"));
  return el("div", { class: "hero-copy" },
    el("p", { class: "hero-kicker", text: kicker }),
    el("h1", { text: t.title }),
    el("p", { class: "meta" }, metaBits(t), t.genres?.length ? [el("span", { class: "sep" }), el("span", { text: genresAZ(t.genres, 2).join(", ") })] : null),
    t.overview ? el("p", { class: "overview", text: t.overview }) : null,
    actions);
}

/** Moves the hero on the page shown (for keys and the remote); null when there is none. */
let heroShow = null;
export const heroStep = (step) => heroShow?.(step);
// On a touch screen the hero's arrows are hidden: a sideways swipe moves it,
// and the tap that ends a swipe doesn't open the title.
let swipe = null;
let swiped = 0;
document.addEventListener("touchstart", (e) => {
  const t = e.touches[0];
  swiped = 0; // a new touch is a new gesture: a tap right after a swipe still counts
  swipe = e.touches.length === 1 && e.target.closest(".hero") ? { x: t.clientX, y: t.clientY } : null;
}, { passive: true });
document.addEventListener("touchend", (e) => {
  if (!swipe) return;
  const t = e.changedTouches[0];
  const dx = t.clientX - swipe.x, dy = t.clientY - swipe.y;
  swipe = null;
  if (Math.abs(dx) > 50 && Math.abs(dx) > Math.abs(dy) * 1.5) { swiped = performance.now(); heroStep(dx < 0 ? 1 : -1); }
}, { passive: true });
document.addEventListener("click", (e) => {
  if (performance.now() - swiped < 400 && e.target.closest(".hero")) { e.stopPropagation(); e.preventDefault(); }
}, true);
let heroKey = null; // the featured title showing, kept across a redraw

function hero(picks, keep = null) {
  clearInterval(heroTimer);
  heroShow = null;
  picks = picks.filter((p) => !isHidden(p.t));
  if (!picks.length) return heroLook(el("section", { class: "hero empty" }));
  const bgUrl = (p) => `url("${p.t.backdrop.replace(/"/g, "%22")}")`;
  let at = Math.max(0, picks.findIndex((p) => p.t.key === keep));
  heroKey = picks[at].t.key;
  const bgA = el("div", { class: "hero-bg", style: `background-image:${bgUrl(picks[at])}` });
  const bgB = el("div", { class: "hero-bg out" });
  const section = heroLook(el("section", { class: "hero" }, bgA, bgB, heroCopy(picks[at])));
  section.lastChild.classList.add("first"); // the copy a kept tab comes back with (styles.css)
  section.picks = picks.map((p) => p.t.key).join("|");
  let front = bgA, back = bgB;
  // A press anywhere on the banner opens the title, like a card does. A drag
  // (to move between titles) or a press on a button doesn't.
  let pressX = null;
  section.addEventListener("pointerdown", (e) => { pressX = e.target.closest("button, a") ? null : e.clientX; });
  section.addEventListener("click", (e) => {
    const still = pressX != null && Math.abs(e.clientX - pressX) < 8;
    pressX = null;
    if (still && !e.target.closest("button, a")) openDetail(picks[at].t);
  });
  section.resume = () => { heroShow = null; };
  if (picks.length < 2) return section;
  const dots = el("div", { class: "hero-dots" }, picks.map((_, i) => el("button", { class: i === at ? "on" : "", "aria-label": `Show featured title ${i + 1}`, onclick: () => show(i) })));
  // Arrows at the sides, a swipe or drag, or the arrow keys move between titles.
  const side = (cls, label, path, step) => el("button", { class: `hero-arrow ${cls}`, "aria-label": label, onclick: () => show(at + step) }, icon(path));
  // The arrows sit with the dots, bottom right, never over the words.
  section.append(el("div", { class: "hero-nav" }, side("prev", "Previous title", ICON.left, -1), dots, side("next", "Next title", ICON.right, 1)));
  let downX = null;
  section.addEventListener("pointerdown", (e) => { if (!e.target.closest("button, a")) downX = e.clientX; });
  section.addEventListener("pointerup", (e) => {
    if (downX == null) return;
    const dx = e.clientX - downX;
    downX = null;
    if (Math.abs(dx) > 60) show(at + (dx < 0 ? 1 : -1));
  });
  section.addEventListener("keydown", (e) => {
    if (e.key === "ArrowLeft" && e.target.closest(".hero-arrow")) { e.preventDefault(); show(at - 1); }
    if (e.key === "ArrowRight" && e.target.closest(".hero-arrow")) { e.preventDefault(); show(at + 1); }
  });
  // Its rotation and keys, started again when a kept tab comes back.
  section.resume = () => {
    heroShow = (step) => document.body.contains(section) && show(at + step);
    restart();
  };
  function show(i) {
    at = (i + picks.length) % picks.length;
    heroKey = picks[at].t.key;
    back.style.backgroundImage = bgUrl(picks[at]);
    back.classList.remove("out");
    front.classList.add("out");
    [front, back] = [back, front];
    section.querySelector(".hero-copy").replaceWith(heroCopy(picks[at]));
    [...dots.children].forEach((d, j) => d.classList.toggle("on", j === at));
    restart();
  }
  function restart() {
    clearInterval(heroTimer);
    heroTimer = setInterval(() => { if (!document.hidden && $("#player").hidden && !$("#detail").open && !preview && document.body.contains(section)) show(at + 1); }, 9000);
  }
  section.resume();
  return section;
}

/** Midnight's hero is shorter; Classic's is a card inside the page, like a banner. */
function heroLook(section) {
  const s = styleNow("watch");
  if (s === "midnight") section.classList.add("compact");
  if (s === "classic") section.classList.add("cl-hero");
  return section;
}

/* ---------- Midnight: the ranking down the side ---------- */
const RANKS = [["top10", "Top 10"], ["trending", "Trending"], ["popular-series", "Series"], ["anime-trending", "Anime"]];

function rankItem(t, i) {
  const b = el("button", { class: "rk", style: `--i:${i}`, onclick: () => { hidePreview(true); open(t); } },
    el("span", { class: "rk-n", text: String(i + 1).padStart(2, "0") }),
    t.poster ? el("img", { class: "rk-img", ...picProps(t.poster, [92, 154], 1.5, "46px"), alt: "", loading: "lazy", decoding: "async" }) : el("span", { class: "rk-img" }),
    el("span", { class: "rk-text" },
      el("strong", { text: t.title }),
      el("small", { text: [t.year, ...genresAZ(t.genres || [], 2)].filter(Boolean).join(" · ") })),
    t.rating ? el("span", { class: "rk-score", text: `★ ${(+t.rating).toFixed(1)}` }) : null);
  return b;
}

/** Lists for the side ranking, from the rows the page already has. */
function rankLists(rows) {
  return RANKS.map(([id, label]) => [label, (rows.find((r) => r.id === id)?.tiles || []).slice(0, 10)]).filter(([, l]) => l.length >= 5).slice(0, 3);
}

function rankSide(rows) {
  const lists = rankLists(rows);
  if (!lists.length) return null;
  const head = el("div", { class: "tabbed-tabs", role: "tablist" });
  const body = el("div", { class: "tabbed-body" });
  let swapT = 0;
  const pick = (i, first) => {
    [...head.children].forEach((b, j) => { b.classList.toggle("on", j === i); b.setAttribute("aria-selected", String(j === i)); });
    const fill = () => { body.replaceChildren(...lists[i][1].map(rankItem)); void body.offsetWidth; body.classList.remove("swap"); };
    if (first) { fill(); return; }
    clearTimeout(swapT);
    body.classList.add("swap");
    swapT = setTimeout(fill, 160);
  };
  head.append(...lists.map(([label], i) => el("button", { role: "tab", onclick: () => pick(i) }, label)));
  pick(0, true);
  const side = el("aside", { class: "mn-side mw-side" }, el("h2", { class: "mn-h", text: "Popular" }), head, body);
  side._sig = JSON.stringify(lists.map(([l, ts]) => [l, ts.map((t) => t.key)]));
  return side;
}

/** The page while it loads: the shape of what's coming (a hero with its
 *  title and buttons, then rows of posters across the whole width), with a
 *  soft light passing over it. */
function skeleton() {
  const bar = (cls) => el("div", { class: `skel ${cls}` });
  const row = (r) => el("section", { class: "row skel-row", style: `--r:${r}` },
    el("div", { class: "row-head" }, bar("skel-head")),
    el("div", { class: "track" }, Array.from({ length: 14 }, (_, i) => el("div", { class: "skel skel-tile", style: `--i:${i}` }))));
  const hero = el("section", { class: "skel-hero", "aria-hidden": "true" },
    el("div", { class: "skel-glow" }),
    el("div", { class: "skel-copy" }, bar("skel-kicker"), bar("skel-title"), bar("skel-title short"), bar("skel-line"), bar("skel-line short"),
      el("div", { class: "skel-btns" }, bar("skel-btn"), bar("skel-btn ghost"))));
  return [hero, el("div", { class: "rows", "aria-busy": "true" }, row(0), row(1), row(2))];
}

async function emptyState(view, tab, seq) {
  const box = el("div", { class: "empty-state" });
  let ms;
  try { ms = await invoke("media_status"); } catch (err) {
    box.append(el("h2", { text: "Couldn't load this page" }), el("p", { text: String(err) }), el("button", { class: "btn primary", onclick: () => renderTab(view, tab) }, "Try again"));
    return box;
  }
  if (!ms.signed_in && tab === "downloads") {
    box.append(el("h2", { text: "Your library lives in Jellyfin" }), el("p", { text: "Sign in to Jellyfin and your movies and series show up here, ready to play." }), el("a", { class: "btn primary", href: "#settings/account" }, "Set up Jellyfin"));
  } else if (tab === "downloads") {
    box.append(el("h2", { text: "Nothing downloaded yet" }), el("p", { text: "Pick something from Home and press Download." }), el("a", { class: "btn primary", href: "#home" }, "Find something"));
  } else {
    box.append(el("h2", { text: "Getting the catalog ready" }), el("p", { text: "Chillarr is gathering movies, series and anime for the first time. This takes a minute; the page fills in on its own." }));
    if (seq === renderSeq) emptyTimer = setTimeout(() => window.dispatchEvent(new Event("chillarr:refresh")), 20000);
  }
  return box;
}

/** A grid of titles, each with a Stop button (or `label`) under it. `stop` resolving to null means "not after all". */
function stopSection(cls, title, note, items, stop, label = "Stop") {
  const count = el("span", { class: "count", text: String(items.length) });
  const grid = el("div", { class: "grid-lib waiting-grid" });
  const section = el("section", { class: `waiting ${cls}` },
    el("div", { class: "row-head" }, el("h2", { text: title }), count),
    note ? el("p", { class: "note waiting-note", text: note }) : null,
    grid);
  grid.append(...items.map((w, j) => {
    const b = tile(w.tile);
    b.style.setProperty("--j", Math.min(j, 18));
    // No `stop`: someone who may not stop or remove these just sees them.
    if (!stop) return el("div", { class: "wait-item" }, b);
    const btn = el("button", { class: "btn small soft stop-wait" }, label);
    const item = el("div", { class: "wait-item" }, b, btn);
    btn.addEventListener("click", async () => {
      btn.disabled = true;
      try {
        const msg = await stop(w);
        if (msg == null) { btn.disabled = false; return; }
        toast(msg);
        item.classList.add("gone");
        setTimeout(() => { item.remove(); count.textContent = String(grid.children.length); if (!grid.children.length) section.remove(); }, 250);
      } catch (e) { toast(String(e), true); btn.disabled = false; }
    });
    return item;
  }));
  return section;
}

/** Stop on a download. A series' (or anime's) asks first whether to stop
 *  just that download or the whole show: every episode download, and
 *  nothing more fetched for it. Closing the question stops nothing. */
async function stopDownload(d) {
  let whole = false;
  if (d.app === "sonarr") {
    const pick = await choose(`Stop ${d.tile.title}?`,
      "The whole series cancels every episode it's downloading and stops getting new ones. What's already downloaded stays.",
      [], "The whole series", "Just this download");
    if (pick == null) return null;
    whole = pick === "yes";
  }
  return invoke("media_stop_download", { which: d.app, downloadId: d.download_id, whole });
}

/** Downloading now on My downloads: one entry per torrent, each with Stop. Kept current. */
async function fillDownloads(view, seq) {
  const slot = view.querySelector(".downloads-slot");
  if (!slot) return;
  const list = await invoke("media_downloads").catch(() => null);
  if (seq !== renderSeq || !list) return;
  const sig = sigFor(view, ".downloads-slot", list);
  if (sig == null) return;
  const section = list.length
    ? stopSection("downloads", "Downloading now", null, list, canDelete() && stopDownload)
    : el("div");
  section.classList.add("downloads-slot");
  section._sig = sig;
  put(view.querySelector(".downloads-slot"), section, seq);
}

/** Watch now titles, each with Remove (My downloads and My List). */
async function fillWatchNow(view, seq) {
  const slot = view.querySelector(".watchnow-slot");
  if (!slot) return;
  const list = await invoke("watchnow_list").catch(() => []);
  if (seq !== renderSeq) return;
  const sig = sigFor(view, ".watchnow-slot", list);
  if (sig == null) return;
  const now = view.querySelector(".watchnow-slot");
  if (!list.length) { now.replaceWith(Object.assign(el("div", { class: "watchnow-slot" }), { _sig: sig })); return; }
  const remove = async (w) => ((await confirmRemove(w.tile)) ? invoke("watchnow_remove", { key: w.tile.key }) : null);
  const section = stopSection("watchnow", "Watch now", "Playing while they download. Remove one to delete what's downloaded so far.", list.map((tile) => ({ tile })), remove, "Remove");
  section.classList.add("watchnow-slot");
  section._sig = sig;
  put(now, section, seq);
}

/** The waiting list: what Radarr/Sonarr are still watching for, each with Stop. */
async function fillWaiting(view, seq) {
  const slot = view.querySelector(".waiting-slot");
  if (!slot) return;
  const list = await invoke("media_waiting").catch(() => []);
  if (seq !== renderSeq) return;
  const sig = sigFor(view, ".waiting-slot", list);
  if (sig == null) return;
  const slotNow = view.querySelector(".waiting-slot");
  const section = stopSection("enter", "Waiting list", "Chillarr keeps looking for these. The moment a copy is out, it downloads it and lets you know.",
    list.filter((w) => !(w.tile.status || "").startsWith("Downloading")), canDelete() && ((w) => invoke("media_stop_waiting", { which: w.app, id: w.id })));
  section.classList.add("waiting-slot");
  section._sig = sig;
  if (section.querySelector(".wait-item")) put(slotNow, section, seq);
  else slotNow.replaceWith(Object.assign(el("div", { class: "waiting-slot" }), { _sig: sig }));
}

/** A My List tile. One that isn't in the library says so on its poster (or where its download is). */
function listTile(t, j) {
  const b = tile(t);
  b.style.setProperty("--j", Math.min(j, 18));
  if (!t.jf_id && !t.status) b.querySelector(".art").append(el("div", { class: "over not-dl", text: "Not downloaded" }));
  return b;
}

/** A Completed tile: what's completed under the title; on the poster, "Deleted", when the clean-up takes it, or "Watched". */
function completedTile(t, j) {
  const b = tile(t);
  b.style.setProperty("--j", Math.min(j, 18));
  if (t.mark) b.querySelector(".art").append(el("div", { class: "over not-dl", text: t.mark }));
  b.append(el("div", { class: "cap done", title: t.completed, text: t.completed }));
  return b;
}

let completedGone = null;

/** My List > Completed: everything finished, still here or deleted. */
function completedSection(list) {
  if (!list.length) return null;
  const grid = el("div", { class: "grid-lib completed-grid" }, list.map(completedTile));
  const section = el("section", { class: "completed" },
    el("div", { class: "row-head" }, el("h2", { text: "Completed" })),
    el("p", { class: "note completed-note", text: "Everything you finished. Deleted ones can be downloaded again any time." }),
    grid);
  // The last one taken off: the section goes too. One listener, for the section drawn last.
  window.removeEventListener("chillarr:completed", completedGone);
  completedGone = () => { if (!grid.querySelector(".tile")) { section.remove(); window.removeEventListener("chillarr:completed", completedGone); } };
  window.addEventListener("chillarr:completed", completedGone);
  return section;
}

/** Completed takes longer to work out than My List: it's filled in once the page is up. */
async function fillCompleted(page, seq) {
  const list = await invoke("media_completed").catch(() => []);
  const slot = page.querySelector(".completed-slot");
  if (seq !== renderSeq || !slot) return;
  const sig = sigFor(page, ".completed-slot", list);
  if (sig == null) return;
  if (!list.length) { slot.replaceWith(Object.assign(el("div", { class: "completed-slot" }), { _sig: sig })); return; }
  // An empty list's big message gets smaller once there's something under it.
  page.querySelector(".empty-state")?.style.setProperty("padding", "4vh 0 2vh");
  const section = completedSection(list);
  section.classList.add("completed-slot");
  section._sig = sig;
  put(slot, section, seq);
}

/** My List: what this profile saved, downloaded or not, then what it completed. Each profile has its own. */
async function myListPage(rows, seq) {
  const all = rows.find((r) => r.id === "saved")?.tiles || [];
  const me = (await invoke("profiles_list").catch(() => [])).find((p) => p.active);
  const groups = { all, ready: all.filter((t) => t.jf_id), notyet: all.filter((t) => !t.jf_id) };
  const names = { all: "All", ready: "Downloaded", notyet: "Not downloaded" };
  // One row, like Reading's My List: Show all opens it into the whole list.
  let grid = el("div");
  const chips = el("div", { class: "chips" }, Object.keys(groups).map((k) => el("button", { class: "chip", dataset: { key: k }, onclick: () => draw(k) }, `${names[k]} ${groups[k].length}`)));
  function draw(key) {
    const section = rowEl({ id: "saved", title: names[key] === "All" ? "Saved for later" : names[key], tiles: [] });
    const track = section.querySelector(".track");
    track.replaceChildren(...groups[key].map(listTile));
    section.querySelector(".count").textContent = String(groups[key].length);
    expandRow(section, "mylist");
    if (grid.parentNode) { unwatchFits(grid.querySelector(".track")); grid.replaceWith(section); }
    grid = section;
    chips.querySelectorAll(".chip").forEach((c) => c.classList.toggle("on", c.dataset.key === key));
  }
  const lead = me ? `${me.name}'s list. Other profiles have their own.` : "Titles you saved for later.";
  if (seq !== renderSeq) return null;
  draw("all");
  const page = el("div", { class: "enter" },
    el("div", { class: "lib-head" }, el("div", {}, el("h1", { text: "My List" }), el("p", { text: lead })), all.length ? chips : null),
    all.length ? grid : el("div", { class: "empty-state", style: "padding-top:6vh" }, el("h2", { text: "Your list is empty" }), el("p", { text: "Press + on anything to save it here. It doesn't download until you say so." }), el("a", { class: "btn primary", href: "#home" }, "Find something")),
    el("div", { class: "watchnow-slot" }),
    el("div", { class: "completed-slot" }));
  fillWatchNow(page, seq);
  fillCompleted(page, seq);
  return page;
}

/** My downloads: what is downloading, then the whole library as a big grid. */
function libraryPage(rows) {
  const lib = { movies: [], series: [], anime: [] };
  for (const r of rows) {
    if (r.id === "library-movies") lib.movies = r.tiles;
    if (r.id === "library-series") lib.series = r.tiles;
    if (r.id === "library-anime") lib.anime = r.tiles;
  }
  const all = [...lib.movies, ...lib.series, ...lib.anime].sort((a, b) => a.title.localeCompare(b.title));
  const notYet = (rows.find((r) => r.id === "saved")?.tiles || []).filter((t) => !t.jf_id);
  const groups = { all, movies: lib.movies, series: lib.series, anime: lib.anime };
  const names = { all: "All", movies: "Movies", series: "Series", anime: "Anime" };
  const grid = el("div", { class: "grid-lib" });
  const draw = (key) => {
    stopSelecting(); // the cards it was picking from are about to go
    grid.replaceChildren(...groups[key].map((t, j) => { const b = tile(t); b.style.setProperty("--j", Math.min(j, 18)); b._sel = t; return b; }));
    chips.querySelectorAll(".chip").forEach((c) => c.classList.toggle("on", c.dataset.key === key));
  };
  const chips = el("div", { class: "chips" }, Object.keys(groups).map((k) => el("button", { class: "chip", dataset: { key: k }, onclick: () => draw(k) }, `${names[k]} ${groups[k].length}`)));
  // Several at once: pick titles, then Delete them together.
  const pick = canDelete() && all.length > 1 ? selectable(grid, () => [...grid.querySelectorAll(".tile")].map((n) => [n, n._sel]),
    [{ label: "Delete", danger: true, run: deleteMany }]) : null;
  const page = el("div", { class: "enter" },
    el("div", { class: "lib-head" }, el("div", {}, el("h1", { text: "My downloads" }), el("p", { text: `${all.length} titles ready to watch` })), el("div", { class: "lib-tools" }, pick, chips)),
    el("div", { class: "downloads-slot" }),
    el("div", { class: "watchnow-slot" }),
    el("div", { class: "waiting-slot" }),
    notYet.length ? el("section", { class: "waiting mylist-notyet" },
      el("div", { class: "row-head" }, el("h2", { text: "My List" }), el("a", { class: "explore", href: "#mylist" }, "See all", icon(ICON.right))),
      el("p", { class: "note waiting-note", text: "On your list but not downloaded. Open one and press Download to get it." }),
      el("div", { class: "grid-lib waiting-grid" }, notYet.map(listTile))) : null,
    notYet.length && all.length ? el("div", { class: "row-head" }, el("h2", { text: "Ready to watch" })) : null,
    all.length ? grid : el("div", { class: "empty-state", style: "padding-top:6vh" }, el("h2", { text: "Nothing downloaded yet" }), el("p", { text: "Press Download on anything that looks good." }), el("a", { class: "btn primary", href: "#home" }, "Find something")));
  draw("all");
  return page;
}

/** Deletes every title picked on My downloads (shows: every episode), after one question. */
async function deleteMany(list) {
  const names = list.map((t) => t.title);
  const shows = list.filter((t) => !movieLike(t)).length;
  const body = `${shows ? "Shows lose every episode. " : ""}They're deleted from your PC. It can't be undone, but you can always get them again.`;
  if (!(await ask(`Delete ${list.length} title${list.length === 1 ? "" : "s"}?`, body, names, "Delete", "Cancel"))) return false;
  toast(`Deleting ${list.length} title${list.length === 1 ? "" : "s"}...`);
  let ok = 0;
  for (const t of list) {
    try { await invoke("media_delete", { tile: t }); ok++; } catch (e) { toast(`${t.title}: ${e}`, true); }
  }
  if (ok) toast(`Deleted ${ok} title${ok === 1 ? "" : "s"}.`);
  setTimeout(() => window.dispatchEvent(new Event("chillarr:refresh")), 1500);
  return ok > 0;
}

/* ---------- Rows that fill in as you scroll ---------- */
let lazyObserver = null;
let lazyChain = Promise.resolve();

/** Lazy rows ask for their tiles one at a time as they come near, never all at once. */
function watchLazyRows(view, rows, seq) {
  lazyObserver?.disconnect();
  lazyObserver = null;
  const lazy = rows.filter((r) => r.style === "lazy");
  if (!lazy.length) return;
  const byId = new Map(lazy.map((r) => [`row-${r.id}`, r]));
  lazyObserver = new IntersectionObserver((entries) => {
    for (const e of entries) {
      if (!e.isIntersecting) continue;
      lazyObserver.unobserve(e.target);
      const section = e.target;
      lazyChain = lazyChain.then(() => fillLazyRow(section, byId.get(section.id), seq));
    }
  }, { rootMargin: "600px 0px" });
  // Only the ones still waiting: a kept tab's filled rows stay as they are.
  for (const id of byId.keys()) { const s = document.getElementById(id); if (s?.querySelector(".skel")) lazyObserver.observe(s); }
}

async function fillLazyRow(section, r, seq) {
  if (seq !== renderSeq || !section.isConnected) return;
  const tiles = await invoke("media_explore", { category: r.explore, page: 0 }).catch(() => []);
  if (seq !== renderSeq || !section.isConnected) return;
  if (!tiles.length) { section.remove(); return; }
  const n = rowEl({ ...r, tiles, style: null });
  n.style.animation = "none";
  section.querySelectorAll(".track").forEach(unwatchFits);
  section.replaceWith(n);
}

/* ---------- Coming back to a page ---------- */
// `mode` "redraw": the page you're on again (the player closed, a download
// landed). "back": a page you were on before (Back, leaving search). Either way
// the old page stays up until the new one is ready, which goes in without the
// entrance animations, and app.js puts you back where you were before the frame is drawn.
const SLOTS = ["downloads-slot", "watchnow-slot", "waiting-slot", "completed-slot"];
const ENTRANCE = new Set(["rise", "pop", "fade"]);
let quietSeq = -1;

/** Skips the entrance animations under `root` (they'd replay on every redraw). */
export function calm(root) {
  if (!root.getAnimations) return; // Chrome 84; older TVs just play them
  for (const a of root.getAnimations({ subtree: true })) if (ENTRANCE.has(a.animationName)) a.finish();
}

/** A part that fills in later; coming back, it lands without its entrance. */
function put(slot, section, seq) {
  slot.replaceWith(section);
  if (seq === quietSeq) calm(section);
}

/** Puts the new page in. A redraw: parts that fill in later keep what they showed, so nothing shifts. */
export function swapIn(view, nodes, mode) {
  if (mode === "redraw") {
    remember(); // where you are now, in case you moved while it loaded
    for (const cls of SLOTS) {
      const old = view.querySelector(`.${cls}`);
      const slot = nodes.map((n) => (n.matches(`.${cls}`) ? n : n.querySelector(`.${cls}`))).find(Boolean);
      if (old && slot) slot.replaceWith(old);
    }
  }
  view.replaceChildren(...nodes);
  if (mode) calm(view);
}

/* ---------- Kept tabs ---------- */
// Each tab's last page (its nodes and the rows it was drawn from), per
// person. Coming back to a tab puts it up at once, as it was left, with its
// timers and observers started again; then it is asked for again behind the
// scenes and only what changed is redrawn: a card whose facts changed, a row
// that came or went, the hero when its titles changed. Six tabs at most.
const pages = new Map(); // tab -> { kind: "rows" | "list", nodes, rows }
let pagesFor = null; // whose they are, and whether Jellyfin was signed in
let onScreen = null; // the kept page in #view now

function* tracksOf(entry) {
  for (const n of entry.nodes) yield* n.querySelectorAll(".row .track");
}

/** A slot showing `list` already: null (leave it). Otherwise what to mark its new contents with. */
function sigFor(root, sel, list) {
  const sig = JSON.stringify(list);
  return root.querySelector(sel)?._sig === sig ? null : sig;
}

/** True when `rows` make a page (not "nothing here yet"). */
function hasContent(tab, rows) {
  if (tab === "mylist") return true;
  const content = rows.filter((r) => !r.id.startsWith("library") && r.id !== "downloading" && r.id !== "continue");
  return rows.length > 0 && (tab === "downloads" || content.length > 0);
}

function keep(tab, kind, nodes, rows) {
  const entry = { kind, nodes, rows };
  pages.set(tab, entry);
  onScreen = entry;
  return entry;
}

/** The parts of a page that fill in later, asked for again. */
function fillSlots(view, tab, seq) {
  if (tab === "downloads") { fillDownloads(view, seq); fillWatchNow(view, seq); fillWaiting(view, seq); }
  if (tab === "mylist") { fillWatchNow(view, seq); fillCompleted(view, seq); }
}

/** A kept page back on screen: where its rows were, and its timers and observers going again. */
function resume(view, tab, entry, seq) {
  onScreen = entry;
  for (const t of tracksOf(entry)) { if (t._left) t.scrollLeft = t._left; watchFits(t); }
  if (entry.kind === "rows") {
    entry.nodes[0].resume?.();
    entry.nodes[1].querySelector(":scope > .fl-tab")?.resume?.();
    watchLazyRows(view, entry.rows, seq);
  }
  fillSlots(view, tab, seq);
  startRefresh(view, tab, entry, seq);
}

/* ---------- Saved rows ---------- */
// Each tab's last rows, kept across restarts, so a tab paints the moment it's
// opened; the answer that follows redraws only what changed.
const savedKey = (tab) => `chillarr-rows:${personKey()}:${tab}`;

function saveRows(tab, rows, text) {
  try { localStorage.setItem(savedKey(tab), text || JSON.stringify(rows)); } catch (e) { /* full or blocked: it just loads the slow way */ }
}

function savedRows(tab) {
  try {
    const rows = JSON.parse(localStorage.getItem(savedKey(tab)) || "null");
    return Array.isArray(rows) && hasContent(tab, rows) ? rows : null;
  } catch (e) { return null; }
}

/** After starting: every tab's rows, one at a time, so the first visit to each is already current. */
export function warmTabs() {
  setTimeout(async () => {
    const who = personKey();
    for (const tab of ["home", "movies", "series", "anime", "mylist", "downloads"]) {
      if (document.hidden) return;
      const rows = await invoke("media_rows", { tab }).catch(() => null);
      if (who !== personKey()) return;
      if (rows && hasContent(tab, rows)) saveRows(tab, rows);
      await new Promise((r) => setTimeout(r, 400));
    }
  }, 2500);
}

/** What keeps a page current while it's up: the downloads list, and rows with downloads or My List in them. */
function startRefresh(view, tab, entry, seq) {
  clearInterval(refreshTimer);
  if (tab === "downloads") {
    refreshTimer = setInterval(() => { if (!document.hidden && $("#player").hidden) fillDownloads(view, seq); }, 15000);
  } else if (entry.kind === "rows" && entry.rows.some((r) => r.id === "downloading" || r.id === "mylist")) {
    refreshTimer = setInterval(() => {
      if (document.hidden || !$("#player").hidden || $("#detail").open || preview) return;
      refreshPage(view, tab, entry, seq);
    }, 20000);
  }
}

/** Asks for a kept page's rows again and redraws only what changed. */
async function refreshPage(view, tab, entry, seq) {
  const fresh = await invoke("media_rows", { tab }).catch(() => null);
  // Gone meanwhile, or the answer failed: the page stays as it is.
  if (seq !== renderSeq || onScreen !== entry || !fresh) return;
  const text = JSON.stringify(fresh);
  // The rows it shows, as text, made once rather than every 20 seconds.
  entry.text ??= JSON.stringify(entry.rows);
  if (text === entry.text) return;
  entry.text = text;
  if (hasContent(tab, fresh)) saveRows(tab, fresh, text);
  if (entry.kind === "rows" && hasContent(tab, fresh)) { patchRows(view, tab, entry, fresh, seq); return; }
  // A list page (or one that has nothing now): drawn again, kept where you are.
  pages.delete(tab);
  onScreen = null;
  await draw(view, tab, fresh, "redraw", seq);
  if (seq === renderSeq) restore();
}

/** New cards fade in where they land (the entrance animations are for whole pages). */
function fadeIn(n) {
  n.style.animation = "none";
  n.animate?.([{ opacity: 0 }, { opacity: 1 }], { duration: 260, easing: "ease-out" });
}

/** A row's cards, made like `r` says, keeping every card that shows the same thing (and its loaded picture). */
function patchTrack(section, r) {
  const track = section.querySelector(".track");
  const wide = WIDE.has(r.id), top10 = r.style === "top10";
  const old = new Map();
  for (const b of track.children) if (b.dataset.key) old.set(b.dataset.key, b);
  const nodes = r.tiles.map((t, j) => {
    const rank = top10 ? j + 1 : 0;
    const prev = old.get(t.key);
    if (prev && prev._tsig === `${rank}|${JSON.stringify(t)}`) { old.delete(t.key); return prev; }
    const b = tile(t, wide, rank);
    fadeIn(b);
    return b;
  });
  // Moved only where the order changed: a node taken out and put back starts its entrance again.
  const left = track.scrollLeft;
  for (const b of old.values()) b.remove();
  nodes.forEach((b, i) => {
    if (track.children[i] === b) return;
    if (b.parentNode) b.style.animation = "none"; // a card moving along, not arriving
    track.insertBefore(b, track.children[i] || null);
  });
  track.scrollLeft = left;
  const count = section.querySelector(".row-head .count");
  if (count) count.textContent = String(r.tiles.length);
}

/** A kept rows page brought up to `fresh`, in place. */
function patchRows(view, tab, entry, fresh, seq) {
  const [heroNow, box] = entry.nodes;
  const picks = pickHeroes(fresh);
  if (picks.map((p) => p.t.key).join("|") !== (heroNow.picks || "")) {
    const h = hero(picks, heroKey);
    heroNow.replaceWith(h);
    entry.nodes[0] = h;
  }
  const genreIds = (rows) => rows.filter((r) => r.id.startsWith("genre-")).map((r) => r.id).join("|");
  if (genreIds(fresh) !== genreIds(entry.rows)) box.querySelector(".genre-chips")?.replaceWith(chipBar(fresh, tab));
  const sideNow = box.querySelector(".mw-side");
  if (sideNow) { const n = rankSide(fresh); if (!n) sideNow.remove(); else if (n._sig !== sideNow._sig) sideNow.replaceWith(n); }
  const before = new Map(entry.rows.map((r) => [r.id, r]));
  const shown = new Map();
  for (const n of box.children) if (n.dataset.row) shown.set(n.dataset.row, n);
  const want = fresh.map((r, i) => {
    const was = before.get(r.id), sec = shown.get(r.id);
    shown.delete(r.id);
    const same = was && JSON.stringify(was) === JSON.stringify(r);
    if (sec && same) return sec;
    // A row that came up empty when it filled in (fillLazyRow) stays out.
    if (!sec && same && r.style === "lazy") return null;
    // Same row, new cards: only the cards that changed.
    if (sec && was && r.style !== "lazy" && was.style !== "lazy" && r.title === was.title && r.explore === was.explore && r.style === was.style && sec.querySelector(".track")) {
      patchTrack(sec, r);
      return sec;
    }
    const n = rowEl(r, i);
    fadeIn(n);
    if (sec) {
      // Drawn again where it was, at the same sideways scroll.
      const left = sec.querySelector(".track")?.scrollLeft || 0;
      sec.querySelectorAll(".track").forEach(unwatchFits);
      sec.replaceWith(n);
      const t = n.querySelector(".track");
      if (left && t) t.scrollLeft = left;
    }
    return n;
  });
  for (const sec of shown.values()) { sec.querySelectorAll(".track").forEach(unwatchFits); sec.remove(); }
  const order = want.filter(Boolean);
  // The rows in order after the chips and the sign-in tip, moving only what's out of place.
  const fixed = [...box.children].filter((n) => !n.dataset.row);
  order.forEach((n, i) => {
    const at = box.children[fixed.length + i];
    if (at === n) return;
    const left = n.querySelector(".track")?.scrollLeft || 0;
    // A row moving up or down, not arriving: no entrance for it or its cards.
    if (n.parentNode) for (const x of [n, ...n.querySelectorAll(".tile")]) x.style.animation = "none";
    box.insertBefore(n, at || null);
    if (left) n.querySelector(".track").scrollLeft = left;
  });
  entry.rows = fresh;
  watchLazyRows(view, fresh, seq);
  startRefresh(view, tab, entry, seq);
}

const WEEK = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/** The week's answer, shared by the tabs drawn within a minute of each other. */
let weekAsk = null;
function watchWeek() {
  if (!weekAsk || weekAsk.who !== personKey() || performance.now() - weekAsk.at > 60000) {
    weekAsk = { who: personKey(), at: performance.now(), p: invoke("watch_week").catch((e) => { weekAsk = null; throw e; }) };
  }
  return weekAsk.p;
}

/** "Out today", "Out 2 days ago", "Fri 21:00". */
function airText(x, now) {
  const d = new Date(x.air);
  const days = Math.round((new Date(now.getFullYear(), now.getMonth(), now.getDate()) - new Date(d.getFullYear(), d.getMonth(), d.getDate())) / 86400000);
  const time = d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  if (x.aired) return days <= 0 ? "Out today" : days === 1 ? "Out yesterday" : `Out ${days} days ago`;
  return days === 0 ? `Today at ${time}` : days === -1 ? `Tomorrow at ${time}` : `${WEEK[d.getDay()]} at ${time}`;
}

/** Your week (issue #44), in every style: your series and anime by the day
 *  their episodes come out, today first, with the episode and when it's out.
 *  Series and Anime show their own; Home shows both. Movies has none: films
 *  don't come out weekly. Fills in when Sonarr answers, and stays out when
 *  nothing of yours airs this week. */
function yourWeek(tab, seq) {
  if (tab !== "home" && tab !== "series" && tab !== "anime") return null;
  const classic = styleNow("watch") === "classic";
  const sec = el("section", { class: classic ? "cl-block mw-week" : "wk" });
  watchWeek().then((all) => {
    if (seq !== renderSeq) return;
    const list = all.filter((x) => (tab === "anime" ? x.anime : tab === "series" ? !x.anime : true)).filter((x) => !isHidden(x.tile));
    if (!list.length) { sec.remove(); return; }
    const now = new Date();
    const today = now.getDay();
    const onDay = (i) => list.filter((x) => new Date(x.air).getDay() === i).sort((a, b) => a.air - b.air);
    // Today first, then the days after it. Each tab says how many come out that day.
    const order = [0, 1, 2, 3, 4, 5, 6].map((k) => (today + k) % 7);
    const label = (i) => { const n = onDay(i).length; return `${i === today ? "Today" : WEEK[i]}${n ? ` · ${n}` : ""}`; };
    const tabs = order.map((i) => [label(i), () => Promise.resolve(onDay(i))]);
    // It opens on today, or on the next day something of yours comes out.
    const first = Math.max(0, order.findIndex((i) => onDay(i).length));
    const isNew = (x) => x.aired && now - x.air < 86400000;
    const sub = tab === "anime" ? "Your anime by the day new episodes come out." : tab === "series" ? "Your shows by the day new episodes come out." : "Your series and anime by the day new episodes come out.";
    if (classic) {
      const cardFor = (x, i) => {
        const b = tile(x.tile);
        b.classList.add("rise", "cl-card");
        b.style.setProperty("--i", String(i));
        if (isNew(x)) b.querySelector(".art").append(el("span", { class: "cl-up", text: "UP" }));
        if (x.episode) b.append(el("div", { class: "cap wk-ep", text: `${x.episode} · ${airText(x, now)}` }));
        return b;
      };
      sec.append(el("h2", { class: "cl-h", text: "Your week" }), el("p", { class: "cl-sub", text: sub }),
        tabbed("cl-week", tabs, (items) => (items.length
          ? [el("div", { class: "cl-grid" }, items.map(cardFor))]
          : [el("p", { class: "cl-empty", text: "Nothing of yours comes out that day." })]), first));
      return;
    }
    // Cinema and Midnight: a row of wide cards, the episode and its time on each.
    const card = (x, i) => {
      const t = x.tile;
      const src = (t.backdrop || t.poster || "").replace("fillWidth=1600", "fillWidth=640");
      const fit = picProps(src, [300, 480, 640], src === t.poster ? 1.5 : 0.5625, "(max-width: 640px) 240px, 300px");
      const b = el("button", { class: "wk-card", style: `--i:${Math.min(i, 8)}`, "aria-label": `${t.title}${x.episode ? `, ${x.episode}` : ""}`, dataset: { key: t.key } },
        el("div", { class: "wk-art" }, src ? el("img", { ...fit, alt: "", loading: "lazy", decoding: "async" }) : noArt(t),
          isNew(x) ? el("span", { class: "wk-new", text: "NEW" }) : null),
        el("div", { class: "wk-text" },
          el("strong", { text: t.title }),
          el("span", { class: "wk-ep" }, x.episode ? el("b", { text: x.episode }) : null, x.episode_title ? ` ${x.episode_title}` : ""),
          el("small", { class: x.aired ? "wk-when out" : "wk-when", text: airText(x, now) })));
      b.addEventListener("click", () => { hidePreview(true); open(t); });
      markHidden(b, t);
      return b;
    };
    sec.append(el("div", { class: "wk-head" }, el("h2", { text: "Your week" }), el("p", { text: sub })),
      tabbed("wk-days", tabs, (items) => (items.length
        ? [el("div", { class: "wk-track" }, items.map(card))]
        : [el("p", { class: "wk-empty", text: "Nothing of yours comes out that day." })]), first));
  }).catch(() => sec.remove());
  return sec;
}

/** Builds a tab's page from its rows and puts it in (`mode` as for renderTab). */
async function draw(view, tab, rows, mode, seq) {
  if (tab === "mylist") {
    const page = await myListPage(rows, seq);
    if (!page || seq !== renderSeq) return;
    swapIn(view, [page], mode);
    keep(tab, "list", [page], rows);
    return;
  }
  if (!hasContent(tab, rows)) {
    pages.delete(tab);
    const box = await emptyState(view, tab, seq);
    if (seq === renderSeq) swapIn(view, [box], mode);
    return;
  }
  if (tab === "downloads") {
    const page = libraryPage(rows);
    swapIn(view, [page], mode);
    const entry = keep(tab, "list", [page], rows);
    fillSlots(view, tab, seq);
    startRefresh(view, tab, entry, seq);
    return;
  }
  const h = hero(pickHeroes(rows), mode === "redraw" ? heroKey : null);
  const banner = store.media && !store.media.signed_in
    ? el("div", { class: "tip", style: "margin:0 var(--gutter) 28px" }, el("span", { text: "Sign in to Jellyfin to see your own movies and series here and play them." }), el("a", { class: "btn small primary", href: "#settings/account", style: "margin-left:auto" }, "Sign in"))
    : null;
  const side = styleNow("watch") === "midnight" ? rankSide(rows) : null;
  const box = el("div", { class: side ? "rows mn-rows" : "rows" });
  const chips = chipBar(rows, tab);
  const filters = inlineFilters(tab, box, chips);
  box.append(...[side, inlinePicker(), filters, filters ? null : chips, banner, yourWeek(tab, seq), ...rows.map((r, i) => rowEl(r, i))].filter(Boolean));
  swapIn(view, [h, box], mode);
  const entry = keep(tab, "rows", [h, box], rows);
  watchLazyRows(view, rows, seq);
  startRefresh(view, tab, entry, seq);
}

// A style change redraws the same rows in the new look: from the saved rows at
// once, then quietly brought up to date. It used to ask for every row again
// first, which on a big catalogue took seconds, so the new style seemed not to
// apply until another tab was opened.
let restyling = false;
export const markRestyle = () => { restyling = true; };

export async function renderTab(view, tab, mode = null) {
  stopBrowse();
  const seq = renderSeq;
  quietSeq = mode ? seq : -1;
  // Another profile, or Jellyfin signed in or out (the sign-in tip): nothing kept fits.
  const owner = `${personKey()}|${store.media && !store.media.signed_in ? "tip" : ""}|${styleNow("watch")}`;
  if (pagesFor !== owner) { pages.clear(); pagesFor = owner; }
  const kept = pages.get(tab);
  if (kept) {
    quietSeq = seq;
    // `kept` tells styles.css to skip the entrances a node plays when put back
    // (calm() can't stop them before Chrome 84).
    view.classList.add("kept");
    if (kept.nodes[0].parentNode !== view) view.replaceChildren(...kept.nodes);
    calm(view);
    resume(view, tab, kept, seq);
    refreshPage(view, tab, kept, seq);
    return;
  }
  // A redraw was asked for because something changed: the saved rows are older than that.
  const restyle = restyling;
  restyling = false;
  const saved = mode === "redraw" && !restyle ? null : savedRows(tab);
  if (saved) {
    await draw(view, tab, saved, mode, seq);
    if (seq !== renderSeq) return;
    const drawn = pages.get(tab);
    if (drawn) { refreshPage(view, tab, drawn, seq); return; }
    // Saved rows that couldn't be drawn: the usual way.
  }
  if (!mode && !saved) view.replaceChildren(...skeleton());
  let rows;
  try {
    rows = await invoke("media_rows", { tab });
  } catch (err) {
    // A redraw that fails leaves the page as it was.
    if (seq !== renderSeq || mode === "redraw") return;
    // My downloads is the library: Jellyfin not answering yet (it takes ~20-30 s
    // to start) says so and tries again, never "Nothing downloaded yet".
    if (tab === "downloads") {
      view.replaceChildren(el("div", { class: "empty-state" }, el("h2", { text: "Getting your library" }), el("p", { text: "Jellyfin isn't answering yet. This page tries again on its own." }), el("button", { class: "btn primary", onclick: () => renderTab(view, tab) }, "Try now")));
      emptyTimer = setTimeout(() => renderTab(view, tab, "back"), 5000); // "back": the message stays up meanwhile, no skeleton flash
      return;
    }
    view.replaceChildren(el("div", { class: "empty-state" }, el("h2", { text: "Couldn't load this page" }), el("p", { text: String(err) }), el("button", { class: "btn primary", onclick: () => renderTab(view, tab) }, "Try again")));
    return;
  }
  // Something newer (another tab, a search) was drawn while this loaded.
  if (seq !== renderSeq) return;
  if (hasContent(tab, rows)) saveRows(tab, rows);
  await draw(view, tab, rows, mode, seq);
}

/* Remove from Continue watching (mylist.js): the card leaves every
   Continue watching row, the one on screen with a short animation, and the
   kept rows forget it so a redraw doesn't bring it back. */
window.addEventListener("chillarr:forgot", (e) => {
  const key = e.detail;
  for (const entry of pages.values()) {
    for (const r of entry.rows) if (r.id === "continue") r.tiles = r.tiles.filter((t) => t.key !== key);
  }
  for (const b of document.querySelectorAll(`#view [data-row="continue"] .tile[data-key="${CSS.escape(key)}"]`)) dropCard(b);
  for (const entry of pages.values()) {
    if (entry === onScreen) continue;
    for (const n of entry.nodes) for (const b of n.querySelectorAll(`[data-row="continue"] .tile[data-key="${CSS.escape(key)}"]`)) dropCard(b, false);
  }
});

/** Takes a card out of its row: it fades and shrinks, then the cards after it close the gap. The row goes with its last card. */
function dropCard(b, animate = true) {
  const row = b.closest(".row");
  const done = () => {
    b.remove();
    const left = row?.querySelectorAll(".track .tile").length || 0;
    const count = row?.querySelector(".row-head .count");
    if (count) count.textContent = String(left);
    if (row && !left) {
      row.querySelectorAll(".track").forEach(unwatchFits);
      if (animate && row.animate) { const a = row.animate([{ opacity: 1 }, { opacity: 0 }], { duration: 200, fill: "forwards" }); a.onfinish = () => row.remove(); }
      else row.remove();
    }
  };
  if (!animate || !b.animate) { done(); return; }
  const w = b.offsetWidth;
  const gap = parseFloat(getComputedStyle(b.parentElement).columnGap) || 0;
  b.style.pointerEvents = "none";
  const out = b.animate([{ opacity: 1, transform: "none" }, { opacity: 0, transform: "scale(0.86)" }], { duration: 200, easing: "ease-in", fill: "forwards" });
  out.onfinish = () => {
    const shut = b.animate([{ width: `${w}px`, marginRight: "0px" }, { width: "0px", marginRight: `${-gap}px` }], { duration: 260, easing: "cubic-bezier(0.16, 1, 0.3, 1)", fill: "forwards" });
    shut.onfinish = done;
  };
}

/**
 * A big grid that loads the next page (40) as you near the bottom, one
 * request at a time. `fetchPage(page)` returns tiles; an empty page ends it.
 */
export function infiniteGrid(seq, fetchPage, endText) {
  const grid = el("div", { class: "grid-lib" });
  const sentinel = el("div", { class: "sentinel" }, el("span", { class: "spinner" }));
  let page = 0, done = false, count = 0, filling = null;
  const seen = new Set();
  /** Loads one page. False when it couldn't, so the loop stops. */
  async function more() {
    let tiles;
    try {
      tiles = await fetchPage(page);
    } catch (err) {
      if (seq === renderSeq) sentinel.replaceChildren(el("p", { class: "note", text: "Couldn't load more." }), el("button", { class: "btn small soft", onclick: () => { sentinel.replaceChildren(el("span", { class: "spinner" })); fill(); } }, "Try again"));
      return false;
    }
    if (seq !== renderSeq) return false;
    const fresh = tiles.filter((t) => !seen.has(t.key));
    fresh.forEach((t) => seen.add(t.key));
    grid.append(...fresh.map((t, j) => { const b = tile(t); b.style.setProperty("--j", Math.min(j, 18)); return b; }));
    count += fresh.length;
    page++;
    done = !fresh.length;
    if (done) sentinel.replaceChildren(el("p", { class: "note", text: endText(count) }));
    return true;
  }
  // Off screen (a page being built to come back to) it is never "near": that would load every page.
  const near = () => sentinel.isConnected && sentinel.offsetParent !== null && sentinel.getBoundingClientRect().top < innerHeight + 900;
  // One loop at a time. A second caller joins the running one; two loops used to
  // spin on each other without ever yielding, which froze the whole window.
  // `want`: keep going until there are that many cards (as many as when you left).
  function fill(want = 0) {
    filling ??= (async () => {
      try {
        // A tall screen can show a whole page at once; keep going until the end is out of view.
        while (!done && seq === renderSeq && (await more()) && !done && (near() || count < want)) { /* next page */ }
      } finally {
        filling = null;
      }
    })();
    return filling;
  }
  const io = new IntersectionObserver((e) => { if (e[0].isIntersecting) fill(); }, { rootMargin: "900px" });
  io.observe(sentinel);
  exploreObserver?.disconnect();
  exploreObserver = io;
  return { nodes: [grid, sentinel], fill, get count() { return count; } };
}

/** "Explore all": every title of one row. */
export async function renderExplore(view, category, title, mode = null, want = 0) {
  stopBrowse();
  const seq = renderSeq;
  const g = infiniteGrid(seq, (page) => invoke("media_explore", { category, page }),
    (n) => (n ? `That's all ${n} for now. The catalog grows as Chillarr runs.` : "Nothing here yet. The catalog is still filling in."));
  const page = el("div", { class: "enter" },
    el("div", { class: "lib-head" }, el("div", {}, el("button", { class: "back-link", onclick: () => goBack() }, icon(ICON.left), "Back"), el("h1", { text: title }))),
    ...g.nodes);
  if (!mode) { view.replaceChildren(page); g.fill(); return; }
  // Coming back: as many cards as when you left are in before it shows.
  await g.fill(want);
  if (seq === renderSeq) swapIn(view, [page], mode);
}

/** 18+ on in this profile's Movies or Anime filters: R films and ecchi anime show on a person's page. */
function makerAdult() {
  return ["filter-movie", "filter-anime"].some((k) => { try { return adultShows(adultOf(JSON.parse(getPref(k) || "{}"))); } catch (e) { return false; } });
}

/** Everything one director, creator or anime staff member made (makers.rs). */
export async function renderMaker(view, id, name, mode = null, want = 0) {
  stopBrowse();
  const seq = renderSeq;
  const g = infiniteGrid(seq, (page) => (page ? Promise.resolve([]) : invoke("media_maker", { id, adult: makerAdult() })),
    (n) => (n ? (n === 1 ? "That's the only title found." : `That's all ${n} titles found.`) : "Nothing found for this person yet."));
  const page = el("div", { class: "enter maker-page" },
    el("div", { class: "lib-head" }, el("div", {},
      el("button", { class: "back-link", onclick: () => goBack() }, icon(ICON.left), "Back"),
      el("h1", { text: name }),
      el("p", { class: "maker-sub", text: "Everything they made" }))),
    ...g.nodes);
  if (!mode) { view.replaceChildren(page); g.fill(); return; }
  await g.fill(want);
  if (seq === renderSeq) swapIn(view, [page], mode);
}

/* ---------- Filters (the page itself is in filter.js) ---------- */
const KIND_FOR_TAB = { home: "movie", movies: "movie", series: "series", anime: "anime" };

let exploreObserver = null;

/* ---------- Search ---------- */
// Results come in source by source ("search-update" events from search.rs):
// the saved catalogs and your library at once, the Radarr/Sonarr lookups as
// they answer. Cards that stay are kept and glide to their new place, new
// ones pop in, and a newer search only replaces the page once it has results.
let searchSeq = 0;
let sr = null;
const SR_HOLD = 120; // ms a new search waits for your library before drawing
const SR_CATS = [["all", "All"], ["movie", "Movies"], ["series", "Series"], ["anime", "Anime"]];
const reduceMotion = () => matchMedia("(prefers-reduced-motion: reduce)").matches;

/** Movie, series or anime, as the search chips sort them. */
function catOf(t) {
  if (t.kind === "anime" || t.genres?.some((g) => g.toLowerCase() === "anime")) return "anime";
  return t.kind === "movie" ? "movie" : "series";
}

/** What a card shows; a change means the card is rebuilt. */
const sigOf = (t) => [t.jf_id, t.status, t.poster, t.year, t.progress, (t.genres || []).join()].join("|");

function searchTile(t) {
  const b = tile(t);
  if (t.genres?.length) b.append(el("div", { class: "cap genres", text: genresAZ(t.genres, 2).join(", ") }));
  b._sig = sigOf(t);
  return b;
}

/** A smaller copy of a picture: it is only ever shown blurred. */
function smallPic(u) {
  return u.replace(/\/(w1280|w780|w500|original)\//, "/w300/").replace(/fillWidth=\d+/, "fillWidth=480").replace(/fillHeight=\d+/, "fillHeight=360");
}

function buildSearchPage(view) {
  const bgs = [el("div", { class: "sr-bg" }), el("div", { class: "sr-bg" })];
  const page = {
    bgs, bgKey: null,
    term: el("h1", { class: "sr-term" }),
    count: el("span", { class: "sr-count" }),
    looking: el("span", { class: "sr-looking", hidden: true }),
    chips: el("div", { class: "chips sr-chips", role: "group", "aria-label": "Show" }),
    note: el("p", { class: "search-note", hidden: true }),
    empty: el("div", { class: "empty-state sr-empty", hidden: true }),
    grid: el("div", { class: "results" }, Array.from({ length: 14 }, () => el("div", { class: "skel skel-tile" }))),
    cat: "all", last: null, drawn: 0, holdTimer: null,
  };
  page.root = el("div", { class: "sr enter" },
    el("header", { class: "sr-head" }, el("div", {}, page.term, el("p", { class: "sr-sub" }, page.count, page.looking)), page.chips),
    page.note, page.grid, page.empty);
  view.replaceChildren(el("div", { class: "sr-backdrop", "aria-hidden": "true" }, bgs), page.root);
  return page;
}

/** Search results for `term`. Only the newest search may draw. */
export function renderSearch(view, term) {
  stopBrowse();
  // Rising across reloads too: the backend silences anything older than the newest.
  const seq = searchSeq = Math.max(searchSeq + 1, Date.now());
  if (!sr || !sr.root.isConnected) sr = buildSearchPage(view);
  const page = sr;
  page.seq = seq;
  page.started = performance.now();
  page.last = null;
  clearTimeout(page.holdTimer);
  // What's on screen stays, dimmed, until the new results are in.
  page.root.classList.add("stale");
  // R and NC-17 films only once 18+ is on in this profile's movie filters.
  const adult = (() => { try { return adultShows(adultOf(JSON.parse(getPref("filter-movie") || "{}"))); } catch (e) { return false; } })();
  invoke("media_search_live", { term, seq, adult })
    .catch(() => null)
    .then(() => setTimeout(() => {
      // Every source has answered. If nothing was drawn (nothing found, or the
      // last answer's event is still on its way), draw what there is.
      if (page.seq === seq && page.drawn !== seq) drawSearch(page, page.last || { seq, term, tiles: [], waiting: [], failed: [] });
    }, 60));
}

listen("search-update", (e) => {
  const p = e.payload;
  const page = sr;
  if (!page || p.seq !== page.seq || !page.root.isConnected || $("#search").value.trim() !== p.term) return;
  page.last = p;
  // A new search that has nothing yet keeps the old results up.
  if (!p.tiles.length && p.waiting.length) return;
  clearTimeout(page.holdTimer);
  const wait = SR_HOLD - (performance.now() - page.started);
  // Your library usually answers within a few ms of the catalog: drawing both
  // together saves your copies jumping to the front a moment later.
  if (page.drawn !== p.seq && wait > 0 && p.waiting.includes("Jellyfin")) {
    page.holdTimer = setTimeout(() => drawSearch(page, page.last), wait);
    return;
  }
  drawSearch(page, p);
});

function drawSearch(page, p) {
  if (p.seq !== page.seq || !page.root.isConnected) return;
  page.drawn = p.seq;
  page.last = p;
  page.root.classList.remove("stale");
  const counts = { all: p.tiles.length, movie: 0, series: 0, anime: 0 };
  for (const t of p.tiles) counts[catOf(t)]++;
  // A choice with nothing in it (a new term) goes back to All.
  if (page.cat !== "all" && !counts[page.cat] && !p.waiting.length) page.cat = "all";
  const shown = page.cat === "all" ? p.tiles : p.tiles.filter((t) => catOf(t) === page.cat);

  page.term.textContent = `“${p.term}”`;
  page.count.textContent = p.tiles.length ? `${p.tiles.length} ${p.tiles.length === 1 ? "title" : "titles"}` : p.waiting.length ? "Looking" : "No titles";
  const slow = p.waiting.filter((w) => w !== "Chillarr");
  page.looking.hidden = !slow.length;
  page.looking.replaceChildren(el("i"), `Still asking ${slow.join(" and ")}`);
  const used = SR_CATS.filter(([k]) => k === "all" || counts[k]);
  page.chips.replaceChildren(...(used.length > 2 ? used.map(([k, label]) => el("button", {
    class: `chip${page.cat === k ? " on" : ""}`, "aria-pressed": String(page.cat === k),
    onclick: () => { page.cat = k; drawSearch(page, page.last); },
  }, label, el("span", { class: "chip-n", text: String(counts[k]) }))) : []));
  const f = p.failed;
  page.note.hidden = !f.length;
  if (f.length) page.note.textContent = `${f.join(" and ")} ${f.length > 1 ? "aren't" : "isn't"} answering, so some results may be missing. Fix everything in Settings can restart ${f.length > 1 ? "them" : "it"}.`;

  const nothing = !p.tiles.length && !p.waiting.length;
  page.empty.hidden = !nothing;
  if (nothing) page.empty.replaceChildren(
    el("h2", { text: f.length ? "Search isn't working right now" : "Nothing by that name" }),
    el("p", { text: f.length ? "Try again in a moment." : "Check the spelling, or try fewer words." }));
  syncGrid(page.grid, shown, p.waiting.some((w) => w === "Radarr" || w === "Sonarr") ? 6 : 0);
  searchBackdrop(page, shown.find((t) => t.backdrop || t.poster));
}

/** Puts `tiles` in the grid, keeping the cards already there (they glide to their new place). */
function syncGrid(grid, tiles, skeletons) {
  const old = new Map();
  for (const b of grid.querySelectorAll(":scope > .tile")) old.set(b.dataset.key, b);
  const animate = !reduceMotion();
  const before = new Map();
  if (animate) for (const [k, b] of old) before.set(k, b.getBoundingClientRect());
  let fresh = 0;
  const nodes = tiles.map((t) => {
    const prev = old.get(t.key);
    if (prev && prev._sig === sigOf(t)) { old.delete(t.key); prev.classList.add("kept"); return prev; }
    const b = searchTile(t);
    if (prev) {
      // Same title, new facts (a lookup filled in its genres): no entrance, and
      // the picture already shown moves over instead of loading again.
      old.delete(t.key);
      b.classList.add("kept");
      const was = prev.querySelector(".art img"), now = b.querySelector(".art img");
      if (was && now && was.getAttribute("src") === now.getAttribute("src")) now.replaceWith(was);
    } else {
      b.style.setProperty("--j", Math.min(fresh++, 14));
    }
    return b;
  });
  grid.replaceChildren(...nodes, ...Array.from({ length: skeletons }, () => el("div", { class: "skel skel-tile sr-skel" })));
  if (!animate) return;
  // Measure everything first, then animate: one layout, no thrash.
  const moves = [];
  for (const b of nodes) {
    const r0 = before.get(b.dataset.key);
    if (!r0) continue;
    const r1 = b.getBoundingClientRect();
    const dx = r0.left - r1.left, dy = r0.top - r1.top;
    if (Math.abs(dx) > 1 || Math.abs(dy) > 1) moves.push([b, dx, dy]);
  }
  for (const [b, dx, dy] of moves) {
    b.animate([{ transform: `translate(${dx}px, ${dy}px)` }, { transform: "none" }], { duration: 520, easing: "cubic-bezier(0.16, 1, 0.3, 1)" });
  }
}

/** The top result's picture, blurred far back behind the page; crossfades when it changes. */
function searchBackdrop(page, t) {
  const src = t ? smallPic(t.backdrop || t.poster) : null;
  if (src === page.bgKey) return;
  page.bgKey = src;
  const [front, back] = page.bgs;
  if (!src) { front.classList.remove("on"); return; }
  const img = new Image();
  img.decoding = "async";
  decoded(img, src).then(() => {
    if (page.bgKey !== src) return;
    back.style.backgroundImage = `url("${src.replace(/"/g, "%22")}")`;
    back.classList.add("on");
    front.classList.remove("on");
    page.bgs = [back, front];
  }, () => {});
}

store.on((what) => {
  if (what === "added") window.dispatchEvent(new Event("chillarr:refresh"));
});

// A Download being followed: its posters show where it is, live.
listen("get-progress", (e) => {
  const p = e.payload;
  if (p.refresh) {
    if ($("#player").hidden && !$("#detail").open) window.dispatchEvent(new Event("chillarr:refresh"));
    return;
  }
  if (p.gone) {
    // Stopped: back to what the card said, here and on the tabs kept off screen.
    const selector = `.tile[data-key="${CSS.escape(p.key)}"]`;
    const kept = [...pages.values()].flatMap((e) => e.nodes.flatMap((n) => [...n.querySelectorAll(selector)]));
    for (const b of new Set([...document.querySelectorAll(selector), ...kept])) unpaint(b);
    return;
  }
  for (const b of document.querySelectorAll(`.tile[data-key="${CSS.escape(p.key)}"]`)) {
    const box = b.querySelector(".art");
    let over = box.querySelector(".over");
    if (!over) { over = el("div", { class: "over", dataset: { live: "" } }); box.append(over); }
    else if (over.dataset.live === undefined) over.dataset.live = over.textContent;
    over.textContent = p.status;
    if (p.progress != null) {
      let bar = box.querySelector(".bar");
      if (!bar) { bar = el("div", { class: "bar", dataset: { live: "" } }, el("span")); box.append(bar); }
      bar.firstChild.style.width = `${Math.round(p.progress * 100)}%`;
    }
    // Painted over: the next refresh draws this card from what's true, even with no change in the rows.
    b._tsig = "";
  }
});

/** Takes a follower's words and bar off a card: what the card said before, or nothing. */
function unpaint(b) {
  const over = b.querySelector(".over[data-live]");
  if (over) {
    if (over.dataset.live) { over.textContent = over.dataset.live; delete over.dataset.live; }
    else over.remove();
  }
  b.querySelector(".bar[data-live]")?.remove();
}

// My List changed: redraw so its row is current, once the detail panel or
// hover card it changed from has closed (a redraw under them would jump).
let listChanged = false;
let savedInDetail = false;
window.addEventListener("chillarr:saved", () => {
  if ($("#detail").open) savedInDetail = true; // one redraw however many times it changed
  else listChanged = true;
});
$("#detail").addEventListener("close", () => {
  if (!savedInDetail) return;
  savedInDetail = false;
  window.dispatchEvent(new Event("chillarr:refresh"));
});
