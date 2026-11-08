// The reader. Pages come byte for byte from the chapter on the PC (read.rs,
// through the bridge), so what you see is exactly the source's own image, or
// the graphics card's sharpened copy of it when Settings has that on.
//
// Webtoon (manhwa, manhua): one seamless column. Every page's box is sized
// from its real width and height before it loads, so nothing jumps; pages
// load about three screens ahead; the end of a chapter rolls into the next.
// The column starts at the pages' own width (never blown up past it).
// Manga: right to left, a page at a time, two side by side on a wide screen
// when both are upright; tap the sides, swipe, or use the arrow keys;
// double-tap zooms. "Left to right" is the same for western comics. Pages
// start fitted to the screen.
//
// Zoom in every mode: the bar's - and + (click the number to reset), + and -
// on the keyboard, Ctrl and the mouse wheel, or a pinch. Each profile keeps
// its own zoom for each mode.
//
// Runs on the LG TV's Chrome 53 too: no aspect-ratio (padding boxes instead),
// IntersectionObserver (Chrome 51) for loading ahead, no Element.scrollBy
// (Chrome 61; scrolling is eased by hand), CSS transitions on transform and
// opacity only. Motion is cut when the system asks for less.
import { el, icon, ICON, invoke, toast, WEB, getPref, setPref, screenMode, windowFull } from "./api.js";

const MODES = [
  { id: "webtoon", label: "Scroll" },
  { id: "manga", label: "Right to left" },
  { id: "ltr", label: "Left to right" },
];
const PRELOAD = 4;
const HIDE_BARS = 2800;
const STEPS = [0.25, 0.33, 0.5, 0.67, 0.75, 0.9, 1, 1.1, 1.25, 1.5, 1.75, 2, 2.5, 3, 4];
const Z_MIN = STEPS[0];
const Z_MAX = STEPS[STEPS.length - 1];
const calm = () => !!(window.matchMedia && window.matchMedia("(prefers-reduced-motion: reduce)").matches);
const ms = (n) => (calm() ? 0 : n);

let base = null;
async function bridgeBase() {
  if (base) return base;
  base = WEB ? await window.__CHILLARR_BRIDGE__() : await invoke("read_base");
  return base;
}

let r = null; // the open reader's state

/** Opens chapter `chapter` of series `key`. `onClose` runs when it closes. */
export async function openReader(key, chapter, onClose = null) {
  let opened;
  let root;
  try {
    [opened, root] = await Promise.all([invoke("read_open", { key, chapter }), bridgeBase()]);
  } catch (e) {
    base = null;
    toast(String(e), true);
    return;
  }
  if (!r) build(onClose);
  r.key = key;
  load(opened, root);
}

function build(onClose) {
  const box = el("div", { class: "reader opening", tabindex: "-1", role: "dialog", "aria-label": "Reader" });
  const title = el("div", { class: "rd-title" });
  const count = el("span", { class: "rd-count" });
  const hd = el("span", { class: "rd-hd", title: "Sharpened by your graphics card", hidden: true }, "HD");
  const modeBtns = MODES.map((m) => el("button", { class: "rd-mode", dataset: { mode: m.id }, onclick: () => setMode(m.id, true) }, m.label));
  const zLabel = el("button", { class: "rd-zval", title: "Reset zoom", "aria-label": "Reset zoom", onclick: () => zoomReset() });
  const zoomBox = el("div", { class: "rd-zoom", role: "group", "aria-label": "Zoom" },
    el("button", { class: "rd-zbtn", "aria-label": "Zoom out", title: "Zoom out (-)", onclick: () => zoomStep(-1) }, "−"),
    zLabel,
    el("button", { class: "rd-zbtn", "aria-label": "Zoom in", title: "Zoom in (+)", onclick: () => zoomStep(1) }, "+"));
  const top = el("div", { class: "rd-bar rd-top" },
    el("button", { class: "rd-btn", "aria-label": "Close", onclick: close }, icon(ICON.left)),
    title, hd, el("div", { class: "rd-modes", role: "group", "aria-label": "Reading direction" }, modeBtns), zoomBox, count,
    el("button", { class: "rd-btn rd-full", "aria-label": "Full screen", title: "Full screen (F)", onclick: () => toggleFull() }, icon(FULL)));
  const prevCh = el("button", { class: "rd-btn rd-ch", onclick: () => jumpChapter(-1) }, "Previous chapter");
  const nextCh = el("button", { class: "rd-btn rd-ch", onclick: () => jumpChapter(1) }, "Next chapter");
  const slider = el("input", { class: "rd-slider", type: "range", min: "0", max: "0", value: "0", "aria-label": "Page" });
  slider.addEventListener("input", () => goTo(+slider.value, false, 0));
  const bottom = el("div", { class: "rd-bar rd-bottom" }, prevCh, slider, nextCh);
  const fill = el("i");
  const progress = el("div", { class: "rd-progress", "aria-hidden": "true" }, fill);
  const stage = el("div", { class: "rd-stage" });
  box.append(stage, progress, top, bottom);
  document.body.append(box);
  document.body.style.overflow = "hidden";
  r = { box, stage, title, count, hd, slider, prevCh, nextCh, modeBtns, zLabel, fill, onClose, hideT: 0, saveT: 0, io: null, page: 0, z: 1, glide: null, turning: null };
  box.addEventListener("keydown", onKey);
  box.addEventListener("mousemove", onMove);
  box.addEventListener("touchstart", onTouchStart, { passive: true });
  box.addEventListener("touchmove", onTouchMove, { passive: false });
  box.addEventListener("touchend", onTouchEnd, { passive: true });
  box.addEventListener("wheel", onWheel, { passive: false });
  stage.addEventListener("scroll", onScroll, { passive: true });
  stage.addEventListener("click", onStageClick);
  stage.addEventListener("mousedown", onDragStart);
  window.addEventListener("resize", onResize);
  box.focus();
  // Fades in rather than snapping over the page behind.
  requestAnimationFrame(() => requestAnimationFrame(() => box.classList.remove("opening")));
}

function close() {
  if (!r) return;
  saveNow();
  // Full screen that the reader turned on goes off with it.
  if (r.fullByMe) setFull(false);
  r.io?.disconnect();
  r.endIo?.forEach((o) => o.disconnect());
  clearTimeout(r.hideT);
  window.removeEventListener("resize", onResize);
  const { box, onClose: done } = r;
  r = null;
  document.body.style.overflow = "";
  const gone = () => box.remove();
  if (ms(1)) { box.classList.add("opening"); setTimeout(gone, 200); } else gone();
  done?.();
}

/* ---------- a chapter ---------- */

function load(o, root) {
  const first = !r.o;
  r.o = o;
  r.url = (i) => `${root}/read/${o.ticket}/${i}`;
  r.title.replaceChildren(el("strong", { text: o.title }), el("span", { text: chName(o.chapter) }));
  r.hd.hidden = !o.sharp;
  r.slider.max = String(Math.max(0, o.pages.length - 1));
  r.prevCh.disabled = o.prev == null;
  r.nextCh.disabled = o.next == null;
  // A new chapter fades over the old one; the first one draws at once.
  if (first) setMode(o.mode, false, o.page);
  else fade(() => setMode(o.mode, false, o.page));
  showBars();
}

const chName = (c) => (c.number != null ? `Chapter ${c.number}` : c.name || "");

/** The stage fades out, `draw` runs, and it fades back in. */
function fade(draw) {
  const t = ms(140);
  if (!t) { draw(); return; }
  const stage = r.stage;
  stage.classList.add("rd-fading");
  // Keys and taps wait for the new pages.
  r.fading = true;
  setTimeout(() => {
    if (!r || r.stage !== stage) return;
    r.fading = false;
    draw();
    requestAnimationFrame(() => stage.classList.remove("rd-fading"));
  }, t);
}

function setMode(mode, save, page = r.page) {
  const was = r.mode;
  r.mode = MODES.some((m) => m.id === mode) ? mode : "manga";
  r.modeBtns.forEach((b) => b.classList.toggle("on", b.dataset.mode === r.mode));
  if (save) invoke("read_mode", { key: r.key, mode: r.mode }).catch(() => {});
  cancelAnimationFrame(r.zoomRaf);
  r.zTo = null;
  r.z = savedZoom(r.mode);
  const draw = () => {
    if (!r) return;
    r.box.dataset.mode = r.mode;
    r.mode === "webtoon" ? drawScroll(page) : drawPaged(page);
  };
  // Switching between Scroll and the paged modes fades across; a chapter's
  // first draw (no mode yet) and a re-draw in the same mode don't.
  if (save && was && was !== r.mode) fade(draw); else draw();
}

/* ---------- zoom ---------- */

// Scroll: 1 = the pages' own width, or the screen's when that's narrower.
// Paged: 1 = the page fitted to the screen.
function savedZoom(mode) {
  const z = parseFloat(getPref(`reader-zoom-${mode}`) || "");
  return isFinite(z) && z >= 0.1 && z <= 12 ? z : 1;
}

function saveZoom() {
  // Where it's heading, not where an animation has got to.
  if (r) setPref(`reader-zoom-${r.mode}`, String(Math.round((r.zTo ?? r.z) * 100) / 100));
}

/** The pages' own width in screen pixels: the middle one, so a stray wide page doesn't decide. */
function nativeWidth() {
  const ws = r.o.pages.map(([w]) => w).filter((w) => w > 0).sort((a, b) => a - b);
  const w = ws.length ? ws[ws.length >> 1] : 800;
  // A sharpened copy is drawn at twice the size: shown at the original's size, just sharper.
  return r.o.sharp ? w / 2 : w;
}

/** Scroll's column at zoom 1: the pages' own width, never wider than the screen. */
function fitWidth() {
  return Math.min(nativeWidth(), r.stage.clientWidth) || 1;
}

function colWidth(z) {
  return Math.round(Math.max(160, Math.min(fitWidth() * z, r.stage.clientWidth * 3)));
}

/** The zoom as the bar shows it: Scroll's against the pages' own width. */
function shownZoom(z = r.z) {
  return r.mode === "webtoon" ? colWidth(z) / nativeWidth() : z;
}

/** Shown zoom to stored zoom (they differ in Scroll when the screen is narrower than the pages). */
const toZ = (shown) => (r.mode === "webtoon" ? (shown * nativeWidth()) / fitWidth() : shown);

function paintZoom() {
  r.zLabel.textContent = r.mode !== "webtoon" && Math.abs(r.z - 1) < 0.005 ? "Fit" : `${Math.round(shownZoom() * 100)}%`;
}

function zoomStep(dir, fx, fy) {
  // From where a running zoom is heading, so quick presses add up.
  const now = shownZoom(r.zTo ?? r.z);
  let next;
  if (dir > 0) next = STEPS.find((s) => s > now + 0.01) ?? Z_MAX;
  else next = STEPS.slice().reverse().find((s) => s < now - 0.01) ?? Z_MIN;
  zoomTo(toZ(next), fx, fy, true);
  saveZoom();
}

function zoomReset() {
  zoomTo(1, undefined, undefined, true);
  saveZoom();
}

/** Zooms to `z` around the screen point (fx, fy) (the middle when not given). */
function zoomTo(z, fx, fy, animate) {
  if (!r || r.fading) return;
  z = Math.max(toZ(Z_MIN), Math.min(toZ(Z_MAX), z));
  const rect = r.stage.getBoundingClientRect();
  if (fx == null) fx = rect.width / 2; else fx -= rect.left;
  if (fy == null) fy = rect.height / 2; else fy -= rect.top;
  const from = r.z;
  const anchor = takeAnchor(fx, fy);
  cancelAnimationFrame(r.zoomRaf);
  r.zTo = z;
  const t = animate ? ms(180) : 0;
  if (!t || Math.abs(from - z) < 0.001) {
    r.zTo = null;
    applyZoom(z);
    putAnchor(anchor, fx, fy);
    paintZoom();
    return;
  }
  const start = performance.now();
  const frame = (now) => {
    if (!r) return;
    const k = Math.min(1, (now - start) / t);
    const e = 1 - Math.pow(1 - k, 3);
    applyZoom(from + (z - from) * e);
    putAnchor(anchor, fx, fy);
    paintZoom();
    if (k < 1) r.zoomRaf = requestAnimationFrame(frame); else r.zTo = null;
  };
  r.zoomRaf = requestAnimationFrame(frame);
}

function applyZoom(z) {
  r.z = z;
  if (r.mode === "webtoon") {
    if (r.col) r.col.style.width = `${colWidth(z)}px`;
  } else sizeSpread();
}

/** What sits under the screen point, as a share of the thing it's in. */
function takeAnchor(fx, fy) {
  const s = r.stage;
  if (r.mode === "webtoon") {
    if (!r.cells) return null;
    const i = pageAt(s.scrollTop + fy);
    const c = r.cells[i];
    const col = r.col;
    return { i, y: (s.scrollTop + fy - c.offsetTop) / (c.offsetHeight || 1), x: (s.scrollLeft + fx - col.offsetLeft) / (col.offsetWidth || 1) };
  }
  const sp = r.spread;
  if (!sp) return null;
  return { x: (s.scrollLeft + fx - sp.offsetLeft) / (sp.offsetWidth || 1), y: (s.scrollTop + fy - sp.offsetTop) / (sp.offsetHeight || 1) };
}

function putAnchor(a, fx, fy) {
  if (!a) return;
  const s = r.stage;
  if (r.mode === "webtoon") {
    const c = r.cells[a.i];
    const col = r.col;
    s.scrollTop = c.offsetTop + a.y * c.offsetHeight - fy;
    s.scrollLeft = col.offsetLeft + a.x * col.offsetWidth - fx;
    return;
  }
  const sp = r.spread;
  s.scrollLeft = sp.offsetLeft + a.x * sp.offsetWidth - fx;
  s.scrollTop = sp.offsetTop + a.y * sp.offsetHeight - fy;
}

/* ---------- webtoon: one column ---------- */

function drawScroll(page) {
  r.io?.disconnect();
  r.endIo?.forEach((o) => o.disconnect());
  r.endIo = [];
  r.spread = null;
  const o = r.o;
  const col = el("div", { class: "rd-col" });
  const imgs = o.pages.map(([w, h], i) => {
    // A box the page's own shape, so the column never jumps as pages arrive.
    const pad = w && h ? (h / w) * 100 : 141;
    const img = el("img", { alt: `Page ${i + 1}`, decoding: "async", draggable: "false" });
    const cell = el("div", { class: "rd-cell", style: `padding-bottom:${pad}%`, dataset: { i: String(i) } }, img);
    img.addEventListener("load", () => cell.classList.add("loaded"), { once: true });
    img.addEventListener("error", () => cell.classList.add("failed"), { once: true });
    return cell;
  });
  col.append(...imgs, endCard());
  r.stage.replaceChildren(col);
  r.col = col;
  r.cells = imgs;
  col.style.width = `${colWidth(r.z)}px`;
  paintZoom();
  // Load what's near the screen (three screens ahead, one behind), and
  // always the next two pages: webtoon pages can be 5000 px tall, taller
  // than any margin, and the next one must be there before you reach it.
  const want = (i) => { const img = imgs[i]?.firstChild; if (img && !img.src) img.src = r.url(i); };
  r.io = new IntersectionObserver((seen) => {
    for (const s of seen) {
      if (!s.isIntersecting) continue;
      const i = +s.target.dataset.i;
      want(i); want(i + 1); want(i + 2);
    }
  }, { root: r.stage, rootMargin: "100% 0px 300% 0px" });
  imgs.forEach((c) => r.io.observe(c));
  requestAnimationFrame(() => {
    if (!r || r.col !== col) return;
    const target = imgs[Math.min(page, imgs.length - 1)];
    r.stage.scrollTop = target ? target.offsetTop : 0;
    r.stage.scrollLeft = Math.max(0, (col.offsetWidth - r.stage.clientWidth) / 2);
    r.page = page;
    paintCount();
  });
}

function endCard() {
  const o = r.o;
  // Reaching the end is reading it, however short the last page is.
  const seen = new IntersectionObserver(([s]) => {
    if (s.isIntersecting && r?.o === o && r.page !== o.pages.length - 1) { r.page = o.pages.length - 1; paintCount(); saveNow(); }
  }, { root: r.stage, threshold: 0.2 });
  r.endIo.push(seen);
  const watchEnd = (card) => { seen.observe(card); return card; };
  if (o.next == null) return watchEnd(el("div", { class: "rd-end" }, el("p", { text: "You're up to date." }), el("button", { class: "btn soft", onclick: close }, "Close")));
  const card = el("div", { class: "rd-end" }, el("p", { text: "Next chapter" }), el("button", { class: "btn play", onclick: () => jumpChapter(1) }, "Keep reading"));
  // Rolled into view and left there: the next chapter follows on.
  let t = 0;
  const io = new IntersectionObserver(([s]) => {
    clearTimeout(t);
    if (s.isIntersecting && s.intersectionRatio > 0.6) t = setTimeout(() => { if (r?.o === o) jumpChapter(1); }, 1200);
  }, { root: r.stage, threshold: [0, 0.6, 1] });
  r.endIo.push(io);
  io.observe(card);
  return watchEnd(card);
}

/** The page at height `y` in the column. */
function pageAt(y) {
  let lo = 0, hi = r.cells.length - 1;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (r.cells[mid].offsetTop <= y) lo = mid; else hi = mid - 1;
  }
  return lo;
}

function onScroll() {
  if (!r) return;
  calmBars();
  if (r.mode !== "webtoon" || !r.cells) return;
  // The page at the top third of the screen is the one you're on.
  const lo = pageAt(r.stage.scrollTop + r.stage.clientHeight / 3);
  if (lo !== r.page) { r.page = lo; paintCount(); saveSoon(); }
}

/** Eases the column by `dy`; presses while it's moving add up. */
function glideBy(dy) {
  const s = r.stage;
  const maxTop = s.scrollHeight - s.clientHeight;
  const from = s.scrollTop;
  const to = Math.max(0, Math.min(maxTop, (r.glide ? r.glide.to : from) + dy));
  const t = ms(260);
  if (!t) { s.scrollTop = to; return; }
  if (r.glide) cancelAnimationFrame(r.glide.raf);
  const g = { from, to, start: performance.now(), raf: 0 };
  r.glide = g;
  const frame = (now) => {
    if (!r || r.glide !== g) return;
    const k = Math.min(1, (now - g.start) / t);
    const e = k < 0.5 ? 4 * k * k * k : 1 - Math.pow(-2 * k + 2, 3) / 2;
    s.scrollTop = g.from + (g.to - g.from) * e;
    if (k < 1) g.raf = requestAnimationFrame(frame); else r.glide = null;
  };
  g.raf = requestAnimationFrame(frame);
}

/* ---------- manga: a page (or two) at a time ---------- */

function spreads() {
  // Two upright pages side by side when the screen is wide; a wide page
  // (a double spread in the scan) always stands alone.
  const wide = r.stage.clientWidth > r.stage.clientHeight * 1.15;
  const up = (i) => { const [w, h] = r.o.pages[i] || [0, 0]; return !w || h >= w; };
  const out = [];
  for (let i = 0; i < r.o.pages.length;) {
    if (wide && i > 0 && up(i) && i + 1 < r.o.pages.length && up(i + 1)) { out.push([i, i + 1]); i += 2; }
    else { out.push([i]); i += 1; }
  }
  return out;
}

function drawPaged(page) {
  r.io?.disconnect();
  r.endIo?.forEach((o) => o.disconnect());
  r.endIo = [];
  r.cells = null;
  r.col = null;
  r.spreads = spreads();
  r.stage.replaceChildren(el("div", { class: "rd-paged" }));
  r.spread = null;
  goTo(page, true, 0);
}

/** Sizes the shown spread: fitted to the screen, times the zoom. */
function sizeSpread() {
  const sp = r.spread;
  if (!sp) return;
  const imgs = Array.prototype.slice.call(sp.children);
  const dims = imgs.map((im) => r.o.pages[+im.dataset.i] || [0, 0]);
  if (dims.some(([w, h]) => !w || !h)) {
    // Sizes unknown: the browser fits it.
    sp.classList.add("free");
    imgs.forEach((im) => { im.style.height = r.z > 1 ? `${100 * r.z}vh` : ""; im.style.width = ""; });
    return;
  }
  // Each page's width at a height of 1, side by side.
  const ratio = dims.map(([w, h]) => w / h);
  const total = ratio.reduce((a, b) => a + b, 0);
  const fitH = Math.min(r.stage.clientHeight, r.stage.clientWidth / total);
  const h = Math.max(40, fitH * r.z);
  imgs.forEach((im, k) => { im.style.height = `${h}px`; im.style.width = `${ratio[k] * h}px`; });
}

/**
 * Shows the spread holding `page`. `dir` slides it in: +1 onward in the
 * story, -1 back, 0 at once.
 */
function goTo(page, force, dir) {
  if (!r) return;
  page = Math.max(0, Math.min(page, r.o.pages.length - 1));
  if (r.mode === "webtoon") {
    const c = r.cells?.[page];
    if (c) r.stage.scrollTop = c.offsetTop;
    r.page = page;
    paintCount();
    saveSoon();
    return;
  }
  const at = r.spreads.findIndex((s) => s.includes(page));
  const spread = r.spreads[at] || [page];
  if (!force && spread.includes(r.page) && r.spread) { r.page = page; paintCount(); return; }
  r.page = spread[0];
  const order = r.mode === "manga" ? spread.slice().reverse() : spread;
  const holder = r.stage.firstChild;
  const sp = el("div", { class: "rd-spread" + (spread.length === 2 ? " two" : "") },
    ...order.map((i) => el("img", { src: r.url(i), alt: `Page ${i + 1}`, draggable: "false", decoding: "async", dataset: { i: String(i) } })));
  const old = r.spread;
  if (old) {
    // Lifted out where it stands, so the new one takes the middle under it.
    old.style.left = `${old.offsetLeft}px`;
    old.style.top = `${old.offsetTop}px`;
    old.classList.add("leaving");
  }
  r.spread = sp;
  holder.append(sp);
  sizeSpread();
  paintZoom();
  turn(old, sp, dir);
  // A zoomed page starts where reading starts: top right for manga, top left otherwise.
  r.stage.scrollTop = 0;
  r.stage.scrollLeft = r.mode === "manga" ? r.stage.scrollWidth : 0;
  // The next few, so turning is instant.
  for (const s of r.spreads.slice(at + 1, at + 1 + PRELOAD)) for (const i of s) { const im = new Image(); im.src = r.url(i); }
  paintCount();
  saveSoon();
}

/** Slides the old spread out and the new one in, the way the story runs. */
function turn(old, sp, dir) {
  if (r.turning) { r.turning.remove(); r.turning = null; }
  const t = dir ? ms(240) : 0;
  if (!old) return;
  if (!t) { old.remove(); return; }
  // Onward in a right-to-left book comes in from the left.
  const fromLeft = (dir > 0) === (r.mode === "manga");
  old.classList.add(fromLeft ? "to-right" : "to-left");
  sp.classList.add("coming", fromLeft ? "from-left" : "from-right");
  r.turning = old;
  // Waits a moment for the new page, so it doesn't slide in empty.
  const go = () => {
    if (!r || r.spread !== sp) return;
    requestAnimationFrame(() => { sp.classList.remove("coming", "from-left", "from-right"); old.classList.add("gone"); });
    setTimeout(() => { old.remove(); if (r && r.turning === old) r.turning = null; }, t + 40);
  };
  const imgs = Array.prototype.slice.call(sp.querySelectorAll("img"));
  if (imgs.every((i) => i.complete)) { requestAnimationFrame(go); return; }
  let done = false;
  const once = () => { if (!done) { done = true; go(); } };
  imgs.forEach((i) => { i.addEventListener("load", once, { once: true }); i.addEventListener("error", once, { once: true }); });
  setTimeout(once, 160);
}

function step(dir) {
  // dir: +1 = onward in the story.
  if (r.fading) return;
  if (r.mode === "webtoon") { glideBy(dir * r.stage.clientHeight * 0.85); return; }
  const at = r.spreads.findIndex((s) => s.includes(r.page));
  const next = at + dir;
  if (next >= r.spreads.length) { if (r.o.next != null) jumpChapter(1); else toast("That's the last chapter for now."); return; }
  if (next < 0) { if (r.o.prev != null) jumpChapter(-1, true); return; }
  goTo(r.spreads[next][0], true, dir);
}

async function jumpChapter(dir, toEnd = false) {
  const id = dir > 0 ? r.o.next : r.o.prev;
  if (id == null) return;
  saveNow();
  try {
    const o = await invoke("read_open", { key: r.key, chapter: id });
    if (!r) return;
    if (toEnd) o.page = o.pages.length - 1;
    load(o, await bridgeBase());
  } catch (e) { toast(String(e), true); }
}

/* ---------- input ---------- */

let dragged = false;
function onStageClick(e) {
  if (dragged) { dragged = false; return; }
  if (r.mode === "webtoon") { toggleBars(); return; }
  const x = e.clientX / window.innerWidth;
  // Manga reads right to left: the left side goes on.
  const onward = r.mode === "manga" ? x < 0.33 : x > 0.67;
  const back = r.mode === "manga" ? x > 0.67 : x < 0.33;
  if (onward) step(1); else if (back) step(-1); else toggleBars();
}

/** A zoomed page moves with the mouse held down. */
function onDragStart(e) {
  if (e.button !== 0 || !r) return;
  const s = r.stage;
  if (s.scrollWidth <= s.clientWidth + 1 && (r.mode === "webtoon" || s.scrollHeight <= s.clientHeight + 1)) return;
  const x0 = e.clientX, y0 = e.clientY, l0 = s.scrollLeft, t0 = s.scrollTop;
  dragged = false;
  const move = (m) => {
    const dx = m.clientX - x0, dy = m.clientY - y0;
    if (!dragged && Math.abs(dx) + Math.abs(dy) < 6) return;
    dragged = true;
    s.classList.add("dragging");
    s.scrollLeft = l0 - dx;
    if (r.mode !== "webtoon") s.scrollTop = t0 - dy;
  };
  const up = () => {
    window.removeEventListener("mousemove", move);
    window.removeEventListener("mouseup", up);
    s.classList.remove("dragging");
  };
  window.addEventListener("mousemove", move);
  window.addEventListener("mouseup", up);
}

function onWheel(e) {
  if (!r || !e.ctrlKey) return;
  // Ctrl and the wheel zooms the pages, not the window.
  e.preventDefault();
  const now = Date.now();
  if (now - (r.wheelAt || 0) < 90) return;
  r.wheelAt = now;
  zoomStep(e.deltaY < 0 ? 1 : -1, e.clientX, e.clientY);
}

let lastTap = 0;
let touch = null;
let pinch = null;
const gap = (a, b) => Math.sqrt((a.clientX - b.clientX) * (a.clientX - b.clientX) + (a.clientY - b.clientY) * (a.clientY - b.clientY));

function onTouchStart(e) {
  if (!r) return;
  if (e.touches.length === 2) {
    const [a, b] = [e.touches[0], e.touches[1]];
    pinch = { d: gap(a, b) || 1, z: r.z, x: (a.clientX + b.clientX) / 2, y: (a.clientY + b.clientY) / 2, raf: 0, next: r.z };
    touch = null;
    return;
  }
  if (e.touches.length !== 1) { touch = null; return; }
  touch = { x: e.touches[0].clientX, y: e.touches[0].clientY, t: Date.now() };
}

function onTouchMove(e) {
  if (!pinch || !r || e.touches.length !== 2) return;
  e.preventDefault();
  pinch.next = Math.max(Z_MIN, Math.min(Z_MAX, pinch.z * gap(e.touches[0], e.touches[1]) / pinch.d));
  if (pinch.raf) return;
  pinch.raf = requestAnimationFrame(() => {
    if (!pinch || !r) return;
    pinch.raf = 0;
    zoomTo(pinch.next, pinch.x, pinch.y, false);
  });
}

function onTouchEnd(e) {
  if (pinch) {
    if (e.touches.length < 2) { pinch = null; saveZoom(); }
    return;
  }
  if (!touch || !r || r.mode === "webtoon") return;
  const t = e.changedTouches[0];
  const dx = t.clientX - touch.x, dy = t.clientY - touch.y;
  const now = Date.now();
  if (now - lastTap < 300 && Math.abs(dx) < 10 && Math.abs(dy) < 10) {
    // Double-tap: in close, or back to fitted.
    zoomTo(r.z > 1.01 ? 1 : 2, t.clientX, t.clientY, true);
    saveZoom();
    lastTap = 0;
    return;
  }
  lastTap = now;
  if (r.z > 1.01 || Math.abs(dx) < 50 || Math.abs(dx) < Math.abs(dy) * 1.5) return;
  // A swipe pulls the next page in from where it lies.
  const left = dx < 0;
  step(r.mode === "manga" ? (left ? -1 : 1) : (left ? 1 : -1));
}

function onKey(e) {
  const k = e.key || "";
  const code = e.keyCode;
  const manga = r.mode === "manga";
  if (k === "Escape" || k === "Backspace" || code === 461 /* webOS Back */) { e.preventDefault(); close(); return; }
  if (e.target && e.target.tagName === "INPUT" && (k === "ArrowLeft" || k === "ArrowRight")) return;
  if (k === "+" || k === "=" || code === 107 || code === 187) { e.preventDefault(); zoomStep(1); showBars(); return; }
  if (k === "-" || k === "_" || code === 109 || code === 189) { e.preventDefault(); zoomStep(-1); showBars(); return; }
  if (k === "0" || code === 48 || code === 96) { e.preventDefault(); zoomReset(); showBars(); return; }
  if (r.mode === "webtoon") {
    if (k === "ArrowDown" || k === "PageDown" || k === " " || code === 40) { e.preventDefault(); step(1); }
    else if (k === "ArrowUp" || k === "PageUp" || code === 38) { e.preventDefault(); step(-1); }
    else if (k === "ArrowRight" || code === 39) { e.preventDefault(); jumpChapter(1); }
    else if (k === "ArrowLeft" || code === 37) { e.preventDefault(); jumpChapter(-1); }
    else if (k === "Home") { e.preventDefault(); glideBy(-r.stage.scrollTop); }
    else if (k === "End") { e.preventDefault(); glideBy(r.stage.scrollHeight); }
  } else if (k === "ArrowLeft" || code === 37) { e.preventDefault(); step(manga ? 1 : -1); }
  else if (k === "ArrowRight" || code === 39) { e.preventDefault(); step(manga ? -1 : 1); }
  else if (k === " " || k === "PageDown" || k === "ArrowDown" || code === 40) { e.preventDefault(); pagedDown(1); }
  else if (k === "PageUp" || k === "ArrowUp" || code === 38) { e.preventDefault(); pagedDown(-1); }
  else if (k === "Enter" || code === 13) { e.preventDefault(); toggleBars(); }
  if (k === "f" || k === "F" || code === 70) { e.preventDefault(); toggleFull(); }
  // Reading keys (down, space, page turns) leave the bars away: Enter brings them.
}

/** Down on a zoomed page scrolls it first, then turns. */
function pagedDown(dir) {
  const s = r.stage;
  const room = dir > 0 ? s.scrollHeight - s.clientHeight - s.scrollTop : s.scrollTop;
  if (room > 4) glideBy(dir * Math.min(room, s.clientHeight * 0.85));
  else step(dir);
}

let resizeT = 0;
function onResize() {
  clearTimeout(resizeT);
  resizeT = setTimeout(() => {
    if (!r) return;
    if (r.mode === "webtoon") {
      const a = takeAnchor(0, 0);
      applyZoom(r.z);
      putAnchor(a, 0, 0);
      paintZoom();
    } else drawPaged(r.page);
  }, 150);
}

/* ---------- bars, count, saving ---------- */
// The bars stay away while you read. They come back when the mouse really
// moves to the top or bottom of the screen, on a tap in the middle or Enter,
// and go again as soon as you scroll or turn a page. (A still pointer over a
// page scrolling under it fires mousemove too: that brought them back on
// every scroll, user 2026-11-08.)
const EDGE = 120;
let lastXY = null;

function onMove(e) {
  if (!r) return;
  const moved = !lastXY || Math.abs(e.clientX - lastXY[0]) + Math.abs(e.clientY - lastXY[1]) >= 4;
  lastXY = [e.clientX, e.clientY];
  if (!moved || Date.now() - (r.scrolledAt || 0) < 350) return;
  const onBar = e.target && e.target.closest && e.target.closest(".rd-bar");
  if (onBar || e.clientY < EDGE || e.clientY > window.innerHeight - EDGE) showBars();
}

/** Scrolling puts the bars away (unless the pointer is on one). */
function calmBars() {
  r.scrolledAt = Date.now();
  if (!r.box.classList.contains("bars") || Date.now() - (r.shownAt || 0) < 700) return;
  if (r.box.querySelector(".rd-bar:hover")) return;
  clearTimeout(r.hideT);
  r.box.classList.remove("bars");
}

const FULL = "M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5";
async function setFull(on) {
  if (!WEB) { screenMode.app = await windowFull(on); window.dispatchEvent(new Event("chillarr:full")); return screenMode.app; }
  const d = document, x = d.documentElement;
  try {
    if (on) await (x.requestFullscreen || x.webkitRequestFullscreen || (() => {})).call(x);
    else if (d.fullscreenElement || d.webkitFullscreenElement) await (d.exitFullscreen || d.webkitExitFullscreen).call(d);
  } catch (e) { /* the browser said no */ }
  return !!(d.fullscreenElement || d.webkitFullscreenElement);
}
function isFull() { return WEB ? !!(document.fullscreenElement || document.webkitFullscreenElement) : screenMode.app; }
async function toggleFull() {
  if (!r) return;
  const want = !isFull();
  const now = await setFull(want);
  if (r) r.fullByMe = want && now;
  if (r && now) { clearTimeout(r.hideT); r.box.classList.remove("bars"); }
}

function showBars() {
  if (!r) return;
  if (!r.box.classList.contains("bars")) r.shownAt = Date.now();
  r.box.classList.add("bars");
  clearTimeout(r.hideT);
  r.hideT = setTimeout(() => r?.box.classList.remove("bars"), HIDE_BARS);
}
function toggleBars() {
  if (r.box.classList.contains("bars")) { clearTimeout(r.hideT); r.box.classList.remove("bars"); } else showBars();
}

function paintCount() {
  const n = r.o.pages.length;
  const s = r.mode !== "webtoon" && r.spreads ? r.spreads.find((x) => x.includes(r.page)) : null;
  const last = s ? s[s.length - 1] : r.page;
  r.count.textContent = s && s.length === 2 ? `${s[0] + 1}-${s[1] + 1} / ${n}` : `${r.page + 1} / ${n}`;
  r.slider.value = String(r.page);
  // Right to left, the slider and the progress line run that way too.
  r.slider.classList.toggle("rtl", r.mode === "manga");
  r.box.classList.toggle("rtl", r.mode === "manga");
  r.fill.style.transform = `scaleX(${n ? (last + 1) / n : 0})`;
}

function saveSoon() {
  clearTimeout(r.saveT);
  r.saveT = setTimeout(saveNow, 800);
}

function saveNow() {
  if (!r?.o) return;
  clearTimeout(r.saveT);
  const o = r.o;
  // The last page (or the last spread) counts as the chapter read.
  const last = r.mode !== "webtoon" && r.spreads?.length && r.spreads[r.spreads.length - 1].includes(r.page) ? o.pages.length - 1 : r.page;
  invoke("read_progress", { key: r.key, chapter: o.chapter.id, page: last, pages: o.pages.length }).catch(() => {});
}
