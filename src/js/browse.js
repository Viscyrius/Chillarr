// Home, Movies, Series, Anime and My downloads: a hero, genre chips and rows
// of posters; hover previews; Top 10; and "Explore all" pages that keep loading.
import { $, canDelete, el, icon, ICON, invoke, listen, metaBits, store, toast } from "./api.js";
import { openDetail, playShowNow } from "./detail.js";
import { canWatchNow, confirmRemove, isWatchNow, movieLike, play, removeWatchNow, watchNow } from "./player.js";
import { completedActions, confirmRedownload, listable, markWatched, onMyList, removeCompleted, setMyList } from "./mylist.js";
import { goBack, remember } from "./place.js";

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
  exploreObserver?.disconnect();
  exploreObserver = null;
  lazyObserver?.disconnect();
  lazyObserver = null;
  clearTimeout(emptyTimer);
  clearInterval(refreshTimer);
  clearInterval(heroTimer);
  hidePreview(true);
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
  const src = wide ? t.backdrop || t.poster : t.poster || t.backdrop;
  const box = el("div", { class: "art" });
  if (src) {
    const img = el("img", { src, alt: "", loading: "lazy", decoding: "async" });
    img.addEventListener("load", () => img.classList.add("loaded"), { once: true });
    img.addEventListener("error", () => img.replaceWith(noArt(t)));
    box.append(img);
  } else {
    box.append(noArt(t));
  }
  // A Watch now title says where its download is under the title instead (see tile()).
  if (t.status && !isWatchNow(t)) box.append(el("div", { class: "over", text: t.status }));
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
  let quality = "best";
  try { quality = localStorage.getItem("chillarr-quality") || "best"; } catch (e) { /* storage blocked */ }
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
  const b = el("button", { class: cls, "aria-label": t.title, dataset: { key: t.key } }, rank ? el("span", { class: "rank", "aria-hidden": "true", text: String(rank) }) : null, art(t, wide), rank ? null : cap);
  if (isWatchNow(t) && t.status && !rank) b.append(el("div", { class: "cap wn-state", text: t.status }));
  b.addEventListener("click", () => { hidePreview(true); open(t); });
  b.addEventListener("mouseenter", () => schedulePreview(b, t));
  b.addEventListener("mouseleave", () => leaveSoon());
  return b;
}

/* ---------- Hover preview card ---------- */
// The card grows out of the poster (transform + opacity only, on its own
// layer), its picture is fetched and decoded while the pointer is still
// settling, and after a moment it fades into a muted trailer snippet.
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
  return pic ? pic.replace("/w1280/", "/w780/").replace("/original/", "/w780/") : "";
}

const warmed = new Map();
/** Starts fetching and decoding the card's picture before the card opens. */
function warm(t) {
  const src = cardPic(t);
  if (!src || warmed.has(src)) return;
  const img = new Image();
  img.decoding = "async";
  img.src = src;
  warmed.set(src, img.decode().catch(() => {}));
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

function previewCard(t) {
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
  return el("div", { class: "preview", role: "dialog", "aria-label": t.title },
    el("div", { class: "pv-art" }, pic, el("strong", { class: "pv-title", text: t.title })),
    el("div", { class: "pv-body" }, actions,
      meta.length ? el("div", { class: "pv-meta" }, meta) : null,
      t.overview ? el("p", { class: "pv-overview", text: t.overview }) : null,
      t.genres?.length ? el("div", { class: "pv-genres" }, t.genres.slice(0, 3).map((g) => el("span", { text: g }))) : null));
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
  const w = Math.min(Math.max(r.width * 2, 340), 420);
  const left = Math.min(Math.max(12, r.left + r.width / 2 - w / 2), innerWidth - w - 12);
  const card = previewCard(t);
  const h = w * 9 / 16 + (t.overview ? 186 : 120) + (t.genres?.length ? 36 : 0);
  const top = Math.min(Math.max(76, r.top + r.height / 2 - h / 2), innerHeight - h - 12);
  card.style.cssText = `left:${left}px;top:${top}px;width:${w}px`;
  card.addEventListener("mouseenter", () => clearTimeout(leaveTimer));
  card.addEventListener("mouseleave", () => leaveSoon());
  card.addEventListener("click", () => { hidePreview(true); open(t); });
  document.body.append(card);
  // Grow out of the poster: start at the poster's centre and width.
  const dx = r.left + r.width / 2 - (left + w / 2);
  const dy = r.top + r.height / 2 - (top + h / 2);
  card.animate(
    [{ opacity: 0, transform: `translate(${dx}px, ${dy}px) scale(${Math.max(0.5, r.width / w)})` }, { opacity: 1, transform: "none" }],
    { duration: 300, easing: "cubic-bezier(0.16, 1, 0.3, 1)" },
  );
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
    if (!t.isConnected) { fitsWatcher.unobserve(t); continue; }
    t.parentElement.classList.toggle("fits", t.scrollWidth - t.clientWidth < 48);
  }
});

function rowEl(r, i = 0) {
  const wide = WIDE.has(r.id);
  const top10 = r.style === "top10";
  const track = el("div", { class: top10 ? "track top10-track" : "track" }, r.tiles.map((t, j) => { const b = tile(t, wide, top10 ? j + 1 : 0); b.style.setProperty("--j", Math.min(j, 10)); return b; }));
  if (r.style === "lazy") track.append(...Array.from({ length: 8 }, () => el("div", { class: "skel skel-tile" })));
  track.addEventListener("scroll", () => hidePreview(true), { passive: true });
  const step = () => track.clientWidth * 0.85;
  const prev = el("button", { class: "arrow prev", "aria-label": "Scroll left", onclick: () => track.scrollBy({ left: -step(), behavior: "smooth" }) }, icon(ICON.left));
  const next = el("button", { class: "arrow next", "aria-label": "Scroll right", onclick: () => track.scrollBy({ left: step(), behavior: "smooth" }) }, icon(ICON.right));
  fitsWatcher.observe(track);
  const head = el("div", { class: "row-head" }, el("h2", { text: r.title }),
    r.explore ? el("a", { class: "explore", href: exploreHref(r) }, "Explore all", icon(ICON.right)) : el("span", { class: "count", text: String(r.tiles.length) }));
  return el("section", { class: "row", dataset: { row: r.id }, id: `row-${r.id}`, style: `--i:${Math.min(i, 6)}` }, head, el("div", { class: "track-wrap" }, prev, track, next));
}

/** Filters, then genre chips that jump to their row. */
function chipBar(rows, tab) {
  const genres = rows.filter((r) => r.id.startsWith("genre-"));
  const filters = el("button", { class: "chip filter-chip", onclick: () => {
    try { sessionStorage.setItem("chillarr-from", location.hash || "#home"); } catch (e) { /* storage blocked */ }
    location.hash = `#filter/${KIND_FOR_TAB[tab] || "movie"}/{}`;
  } }, icon("M3 5h18M6 12h12M10 19h4"), "Filters");
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
    el("p", { class: "meta" }, metaBits(t), t.genres?.length ? [el("span", { class: "sep" }), el("span", { text: t.genres.slice(0, 2).join(", ") })] : null),
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
  if (!picks.length) return el("section", { class: "hero empty" });
  const bgUrl = (p) => `url("${p.t.backdrop.replace(/"/g, "%22")}")`;
  let at = Math.max(0, picks.findIndex((p) => p.t.key === keep));
  heroKey = picks[at].t.key;
  const bgA = el("div", { class: "hero-bg", style: `background-image:${bgUrl(picks[at])}` });
  const bgB = el("div", { class: "hero-bg out" });
  const section = el("section", { class: "hero" }, bgA, bgB, heroCopy(picks[at]));
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
  if (picks.length < 2) return section;
  const dots = el("div", { class: "hero-dots" }, picks.map((_, i) => el("button", { class: i === at ? "on" : "", "aria-label": `Show featured title ${i + 1}`, onclick: () => show(i) })));
  // Arrows at the sides, a swipe or drag, or the arrow keys move between titles.
  const side = (cls, label, path, step) => el("button", { class: `hero-arrow ${cls}`, "aria-label": label, onclick: () => show(at + step) }, icon(path));
  section.append(dots, side("prev", "Previous title", ICON.left, -1), side("next", "Next title", ICON.right, 1));
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
  heroShow = (step) => document.body.contains(section) && show(at + step);
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
  restart();
  return section;
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

async function emptyState(tab) {
  const ms = await invoke("media_status");
  const box = el("div", { class: "empty-state" });
  if (!ms.signed_in && tab === "downloads") {
    box.append(el("h2", { text: "Your library lives in Jellyfin" }), el("p", { text: "Sign in to Jellyfin and your movies and series show up here, ready to play." }), el("a", { class: "btn primary", href: "#settings/account" }, "Set up Jellyfin"));
  } else if (tab === "downloads") {
    box.append(el("h2", { text: "Nothing downloaded yet" }), el("p", { text: "Pick something from Home and press Download." }), el("a", { class: "btn primary", href: "#home" }, "Find something"));
  } else {
    box.append(el("h2", { text: "Getting the catalog ready" }), el("p", { text: "Chillarr is gathering movies, series and anime for the first time. This takes a minute; the page fills in on its own." }));
    emptyTimer = setTimeout(() => window.dispatchEvent(new Event("chillarr:refresh")), 20000);
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

/** Downloading now on My downloads: one entry per torrent, each with Stop. Kept current. */
async function fillDownloads(view, seq) {
  const slot = view.querySelector(".downloads-slot");
  if (!slot) return;
  const list = await invoke("media_downloads").catch(() => null);
  if (seq !== renderSeq || !list) return;
  const section = list.length
    ? stopSection("downloads", "Downloading now", null, list, canDelete() && ((d) => invoke("media_stop_download", { which: d.app, downloadId: d.download_id })))
    : el("div");
  section.classList.add("downloads-slot");
  put(slot, section, seq);
}

/** Watch now titles, each with Remove (My downloads and My List). */
async function fillWatchNow(view, seq) {
  const slot = view.querySelector(".watchnow-slot");
  if (!slot) return;
  const list = await invoke("watchnow_list").catch(() => []);
  if (seq !== renderSeq) return;
  if (!list.length) { slot.replaceWith(el("div", { class: "watchnow-slot" })); return; }
  const remove = async (w) => ((await confirmRemove(w.tile)) ? invoke("watchnow_remove", { key: w.tile.key }) : null);
  const section = stopSection("watchnow", "Watch now", "Playing while they download. Remove one to delete what's downloaded so far.", list.map((tile) => ({ tile })), remove, "Remove");
  section.classList.add("watchnow-slot");
  put(slot, section, seq);
}

/** The waiting list: what Radarr/Sonarr are still watching for, each with Stop. */
async function fillWaiting(view, seq) {
  const slot = view.querySelector(".waiting-slot");
  if (!slot) return;
  const list = await invoke("media_waiting").catch(() => []);
  if (seq !== renderSeq) return;
  const section = stopSection("enter", "Waiting list", "Chillarr keeps looking for these. The moment a copy is out, it downloads it and lets you know.",
    list.filter((w) => !(w.tile.status || "").startsWith("Downloading")), canDelete() && ((w) => invoke("media_stop_waiting", { which: w.app, id: w.id })));
  section.classList.add("waiting-slot");
  if (section.querySelector(".wait-item")) put(slot, section, seq);
  else slot.replaceWith(el("div", { class: "waiting-slot" }));
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

/** My List > Completed: everything finished, still here or deleted. */
function completedSection(list) {
  if (!list.length) return null;
  const grid = el("div", { class: "grid-lib completed-grid" }, list.map(completedTile));
  const section = el("section", { class: "completed" },
    el("div", { class: "row-head" }, el("h2", { text: "Completed" })),
    el("p", { class: "note completed-note", text: "Everything you finished. Deleted ones can be downloaded again any time." }),
    grid);
  // The last one taken off: the section goes too.
  const gone = () => { if (!grid.querySelector(".tile")) { section.remove(); window.removeEventListener("chillarr:completed", gone); } };
  window.addEventListener("chillarr:completed", gone);
  return section;
}

/** Completed takes longer to work out than My List: it's filled in once the page is up. */
async function fillCompleted(page, seq) {
  const list = await invoke("media_completed").catch(() => []);
  const slot = page.querySelector(".completed-slot");
  if (seq !== renderSeq || !slot) return;
  if (!list.length) { slot.replaceWith(el("div", { class: "completed-slot" })); return; }
  // An empty list's big message gets smaller once there's something under it.
  page.querySelector(".empty-state")?.style.setProperty("padding", "4vh 0 2vh");
  const section = completedSection(list);
  section.classList.add("completed-slot");
  put(slot, section, seq);
}

/** My List: what this profile saved, downloaded or not, then what it completed. Each profile has its own. */
async function myListPage(rows, seq) {
  const all = rows.find((r) => r.id === "saved")?.tiles || [];
  const me = (await invoke("profiles_list").catch(() => [])).find((p) => p.active);
  const groups = { all, ready: all.filter((t) => t.jf_id), notyet: all.filter((t) => !t.jf_id) };
  const names = { all: "All", ready: "Downloaded", notyet: "Not downloaded" };
  const grid = el("div", { class: "grid-lib" });
  const chips = el("div", { class: "chips" }, Object.keys(groups).map((k) => el("button", { class: "chip", dataset: { key: k }, onclick: () => draw(k) }, `${names[k]} ${groups[k].length}`)));
  function draw(key) {
    grid.replaceChildren(...groups[key].map(listTile));
    chips.querySelectorAll(".chip").forEach((c) => c.classList.toggle("on", c.dataset.key === key));
  }
  const lead = me ? `${me.name}'s list. Other profiles have their own.` : "Titles you saved for later.";
  if (seq !== renderSeq) return null;
  const page = el("div", { class: "enter" },
    el("div", { class: "lib-head" }, el("div", {}, el("h1", { text: "My List" }), el("p", { text: lead })), all.length ? chips : null),
    all.length ? grid : el("div", { class: "empty-state", style: "padding-top:6vh" }, el("h2", { text: "Your list is empty" }), el("p", { text: "Press + on anything to save it here. It doesn't download until you say so." }), el("a", { class: "btn primary", href: "#home" }, "Find something")),
    el("div", { class: "watchnow-slot" }),
    el("div", { class: "completed-slot" }));
  draw("all");
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
    grid.replaceChildren(...groups[key].map((t, j) => { const b = tile(t); b.style.setProperty("--j", Math.min(j, 18)); return b; }));
    chips.querySelectorAll(".chip").forEach((c) => c.classList.toggle("on", c.dataset.key === key));
  };
  const chips = el("div", { class: "chips" }, Object.keys(groups).map((k) => el("button", { class: "chip", dataset: { key: k }, onclick: () => draw(k) }, `${names[k]} ${groups[k].length}`)));
  const page = el("div", { class: "enter" },
    el("div", { class: "lib-head" }, el("div", {}, el("h1", { text: "My downloads" }), el("p", { text: `${all.length} titles ready to watch` })), chips),
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

/* ---------- Rows that fill in as you scroll ---------- */
let lazyObserver = null;
let lazyChain = Promise.resolve();

/** Lazy rows ask for their tiles one at a time as they come near, never all at once. */
function watchLazyRows(view, rows, seq) {
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
  for (const id of byId.keys()) { const s = document.getElementById(id); if (s) lazyObserver.observe(s); }
}

async function fillLazyRow(section, r, seq) {
  if (seq !== renderSeq || !section.isConnected) return;
  const tiles = await invoke("media_explore", { category: r.explore, page: 0 }).catch(() => []);
  if (seq !== renderSeq || !section.isConnected) return;
  if (!tiles.length) { section.remove(); return; }
  const n = rowEl({ ...r, tiles, style: null });
  n.style.animation = "none";
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

export async function renderTab(view, tab, mode = null) {
  stopBrowse();
  const seq = renderSeq;
  quietSeq = mode ? seq : -1;
  if (!mode) view.replaceChildren(...skeleton());
  let rows;
  try {
    rows = await invoke("media_rows", { tab });
  } catch (err) {
    // A redraw that fails leaves the page as it was.
    if (seq !== renderSeq || mode === "redraw") return;
    view.replaceChildren(el("div", { class: "empty-state" }, el("h2", { text: "Couldn't load this page" }), el("p", { text: String(err) }), el("button", { class: "btn primary", onclick: () => renderTab(view, tab) }, "Try again")));
    return;
  }
  // Something newer (another tab, a search) was drawn while this loaded.
  if (seq !== renderSeq) return;
  if (tab === "mylist") {
    const page = await myListPage(rows, seq);
    if (page && seq === renderSeq) swapIn(view, [page], mode);
    return;
  }
  const content = rows.filter((r) => !r.id.startsWith("library") && r.id !== "downloading" && r.id !== "continue");
  if (!rows.length || (tab !== "downloads" && !content.length)) {
    const box = await emptyState(tab);
    if (seq === renderSeq) swapIn(view, [box], mode);
    return;
  }
  if (tab === "downloads") {
    swapIn(view, [libraryPage(rows)], mode);
    fillDownloads(view, seq);
    fillWatchNow(view, seq);
    fillWaiting(view, seq);
    refreshTimer = setInterval(() => { if (!document.hidden && $("#player").hidden) fillDownloads(view, seq); }, 15000);
    return;
  }
  const h = hero(pickHeroes(rows), mode === "redraw" ? heroKey : null);
  const banner = store.media && !store.media.signed_in
    ? el("div", { class: "tip", style: "margin:0 var(--gutter) 28px" }, el("span", { text: "Sign in to Jellyfin to see your own movies and series here and play them." }), el("a", { class: "btn small primary", href: "#settings/account", style: "margin-left:auto" }, "Sign in"))
    : null;
  swapIn(view, [h, el("div", { class: "rows" }, chipBar(rows, tab), banner, rows.map((r, i) => rowEl(r, i)))], mode);
  watchLazyRows(view, rows, seq);
  if (rows.some((r) => r.id === "downloading" || r.id === "mylist")) {
    refreshTimer = setInterval(async () => {
      if (document.hidden || !$("#player").hidden || $("#detail").open || preview) return;
      const fresh = await invoke("media_rows", { tab });
      if (seq !== renderSeq) return;
      for (const id of ["mylist", "downloading"]) {
        const d = fresh.find((r) => r.id === id);
        const old = view.querySelector(`[data-row="${id}"]`);
        if (!old) continue;
        const n = d ? rowEl(d) : el("span");
        n.style.animation = "none";
        const left = old.querySelector(".track")?.scrollLeft || 0;
        old.replaceWith(n);
        // Fresh tiles, same sideways scroll.
        if (left) n.querySelector?.(".track")?.scrollTo({ left, behavior: "instant" });
      }
    }, 20000);
  }
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
  const near = () => sentinel.isConnected && sentinel.getBoundingClientRect().top < innerHeight + 900;
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
  return { nodes: [grid, sentinel], fill };
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
  if (t.genres?.length) b.append(el("div", { class: "cap genres", text: t.genres.slice(0, 2).join(", ") }));
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
  invoke("media_search_live", { term, seq })
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
  img.src = src;
  img.decode().then(() => {
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
  for (const b of document.querySelectorAll(`.tile[data-key="${CSS.escape(p.key)}"]`)) {
    const box = b.querySelector(".art");
    let over = box.querySelector(".over");
    if (!over) { over = el("div", { class: "over" }); box.append(over); }
    over.textContent = p.status;
    if (p.progress != null) {
      let bar = box.querySelector(".bar");
      if (!bar) { bar = el("div", { class: "bar" }, el("span")); box.append(bar); }
      bar.firstChild.style.width = `${Math.round(p.progress * 100)}%`;
    }
  }
});

// My List changed: redraw so its row is current, once the detail panel or
// hover card it changed from has closed (a redraw under them would jump).
let listChanged = false;
window.addEventListener("chillarr:saved", () => {
  if ($("#detail").open) $("#detail").addEventListener("close", () => window.dispatchEvent(new Event("chillarr:refresh")), { once: true });
  else listChanged = true;
});
