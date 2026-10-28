// Where you were on each page, so Back, a redraw or closing the player puts
// you on the same spot: the page's scroll, each row's sideways scroll, and the
// card you opened (focused again for controller and keyboard users).
// Places are kept per history entry, so Back to a page finds its own place
// while a fresh click on a tab still starts at the top.
import { $ } from "./api.js";

const saved = new Map(); // history entry id -> place
// The browser's own Back scroll would land before the page is drawn; this file does it.
if ("scrollRestoration" in history) history.scrollRestoration = "manual";
const KEEP = 40;
const TOP = 80; // under the top bar
let here = 0; // the entry on screen
let depth = 0; // how many of our entries are behind it
let opened = null; // the last card opened: { key, row }
let pending = null;

// Each entry gets an id the first time it is shown, kept in history.state so
// Back and Forward bring it back.
let nextId = Date.now();
/** The entry on screen. `fresh` when it was just made (a click), not Back or Forward. */
export function entry() {
  const s = history.state;
  if (s?.at) { here = s.at; depth = s.n || 0; return { id: here, fresh: false }; }
  const n = here ? depth + 1 : 0;
  here = ++nextId;
  depth = n;
  history.replaceState({ ...(s || {}), at: here, n }, "");
  return { id: here, fresh: true };
}

export const current = () => here;

/** Back to the page before, as the browser would, or to where the page was opened from when there's none of ours. */
export function goBack() {
  if (depth > 0) { history.back(); return; }
  let from = "#home";
  try { from = sessionStorage.getItem("chillarr-from") || from; } catch (e) { /* storage blocked */ }
  location.hash = from;
}

// The card you open is the one to come back to.
document.addEventListener("click", (e) => {
  const t = e.target.closest?.("#view .tile[data-key]");
  if (t) opened = describe(t);
}, true);

function describe(e) {
  return { key: e.dataset.key || null, row: e.closest("[data-row]")?.dataset.row || null };
}

function find(d, view = $("#view")) {
  if (!d) return null;
  const row = d.row ? `[data-row="${CSS.escape(d.row)}"]` : "";
  if (d.key) return view.querySelector(`${row} .tile[data-key="${CSS.escape(d.key)}"]`) || (row ? null : view.querySelector(`.tile[data-key="${CSS.escape(d.key)}"]`));
  return row ? view.querySelector(`.row${row}`) : null;
}

/** The first rows (or grid cards) showing under the top bar: what the page is held by. */
function shown(view, n) {
  const out = [];
  // Tiles outside a row's track; :not(.track .tile) in the selector would read
  // better, but Chrome before 88 (older TVs) rejects it and throws.
  for (const e of view.querySelectorAll(".row[data-row], .tile[data-key]")) {
    if (e.classList.contains("tile") && e.closest(".track")) continue;
    const r = e.getBoundingClientRect();
    if (r.bottom > TOP && r.height && out.push(e) >= n) break;
  }
  return out;
}

/** Where `e` sits in the window, ignoring transforms: a row still rising in mustn't count as lower. */
function topOf(e) {
  let y = 0;
  for (let n = e; n; n = n.offsetParent) y += n.offsetTop;
  return y - window.scrollY;
}

/** Remembers where you are on the entry on screen. */
export function remember(id = here) {
  const view = $("#view");
  const rows = {};
  for (const t of view.querySelectorAll(".row[data-row] .track")) if (t.scrollLeft > 0) rows[t.closest(".row").dataset.row] = t.scrollLeft;
  const y = window.scrollY;
  // Held by the card you opened if it's on screen, else by what's under the
  // top bar, so a row appearing above (Continue watching after the player)
  // doesn't push you off it. A few, in case one is gone when you come back.
  const anchors = [];
  if (y > 0) {
    const card = find(opened, view);
    const r = card?.getBoundingClientRect();
    const picks = shown(view, 3);
    if (r && r.bottom > TOP && r.top < innerHeight) picks.unshift(card);
    for (const e of picks) anchors.push({ ...(e.matches(".row") ? { key: null, row: e.dataset.row } : describe(e)), top: topOf(e) });
  }
  saved.delete(id);
  const cards = view.querySelectorAll(".grid-lib > .tile").length;
  saved.set(id, { y, rows, anchors, cards, card: find(opened, view) ? opened : null });
  if (saved.size > KEEP) saved.delete(saved.keys().next().value);
}

export const has = (id = here) => saved.has(id);
/** How many cards a grid page (Explore, Filters) had loaded, to load as many again. */
export const cards = (id = here) => saved.get(id)?.cards || 0;

/* ---------- Putting you back ---------- */
// The page fills in as its data arrives (rows, lazy rows, more pages), so each
// change to it gets another go until everything is back, for a few seconds at
// most. It runs as the change lands, before the frame is drawn, so nothing jumps.
const watcher = new MutationObserver(() => settle());

/** Puts you back where you were on entry `id`. False when there's nothing saved. */
export function restore(id = here) {
  stop();
  const p = saved.get(id);
  if (!p) return false;
  pending = { ...p, rows: { ...p.rows }, focused: !p.card, placed: false, ending: false, timer: setTimeout(stop, 4000) };
  watcher.observe($("#view"), { childList: true, subtree: true });
  settle();
  return true;
}

/** Stops putting you back (you moved, or another page is coming). */
export function stop() {
  if (!pending) return;
  clearTimeout(pending.timer);
  pending = null;
  watcher.disconnect();
}

const navMode = () => document.body.classList.contains("nav-on") || document.body.classList.contains("pad-mode");

function settle() {
  const p = pending;
  if (!p) return;
  const view = $("#view");
  if (view.querySelector(".skel-hero, .rows[aria-busy]")) return; // still loading
  for (const [id, left] of Object.entries(p.rows)) {
    const t = view.querySelector(`.row[data-row="${CSS.escape(id)}"] .track`);
    if (!t || t.querySelector(".skel")) continue;
    t.scrollLeft = left;
    // As far as it goes when the row came back shorter.
    if (Math.abs(t.scrollLeft - left) < 2 || t.scrollLeft >= t.scrollWidth - t.clientWidth - 1) delete p.rows[id];
  }
  let at = null, a = null;
  for (const x of p.anchors) if ((a = find(x, view))) { at = x; break; }
  if (a) {
    const d = topOf(a) - at.top;
    if (Math.abs(d) >= 1) window.scrollBy({ top: d, behavior: "instant" });
    p.placed = Math.abs(topOf(a) - at.top) < 2;
  } else {
    // Nothing that held the page is back: the plain scroll.
    window.scrollTo({ top: p.y, behavior: "instant" });
    p.placed = Math.abs(window.scrollY - p.y) < 2;
  }
  if (p.placed && !p.focused) {
    const c = find(p.card, view);
    const act = document.activeElement;
    const busy = act?.matches("input, textarea, select") || act?.closest("dialog[open]");
    if (c) {
      // Only with a controller or the keyboard, and never out of the search box.
      if (navMode() && !busy) c.focus({ preventScroll: true });
      p.focused = true;
    }
  }
  // Placed: a moment more to hold it while late parts (a lazy row) land.
  if (p.placed && p.focused && !Object.keys(p.rows).length && !p.ending) {
    p.ending = true;
    clearTimeout(p.timer);
    p.timer = setTimeout(stop, 600);
  }
}

// You moved: you're where you want to be now.
for (const ev of ["wheel", "touchstart", "pointerdown", "keydown"]) window.addEventListener(ev, (e) => { if (e.isTrusted) stop(); }, { capture: true, passive: true });
