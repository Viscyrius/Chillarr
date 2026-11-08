// The reader. Pages come byte for byte from the chapter on the PC (read.rs,
// through the bridge), so what you see is exactly the source's own image.
//
// Webtoon (manhwa, manhua): one seamless column. Every page's box is sized
// from its real width and height before it loads, so nothing jumps; pages
// load about three screens ahead; the end of a chapter rolls into the next.
// Manga: right to left, a page at a time, two side by side on a wide screen
// when both are upright; tap the sides, swipe, or use the arrow keys;
// double-tap zooms. "Left to right" is the same for western comics.
//
// Runs on the LG TV's Chrome 53 too: no aspect-ratio (padding boxes instead),
// IntersectionObserver (Chrome 51) for loading ahead.
import { el, icon, ICON, invoke, toast, WEB } from "./api.js";

const MODES = [
  { id: "webtoon", label: "Scroll" },
  { id: "manga", label: "Right to left" },
  { id: "ltr", label: "Left to right" },
];
const PRELOAD = 4;
const HIDE_BARS = 2800;

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
  const box = el("div", { class: "reader", tabindex: "-1", role: "dialog", "aria-label": "Reader" });
  const title = el("div", { class: "rd-title" });
  const count = el("span", { class: "rd-count" });
  const modeBtns = MODES.map((m) => el("button", { class: "rd-mode", dataset: { mode: m.id }, onclick: () => setMode(m.id, true) }, m.label));
  const top = el("div", { class: "rd-bar rd-top" },
    el("button", { class: "rd-btn", "aria-label": "Close", onclick: close }, icon(ICON.left)),
    title, el("div", { class: "rd-modes", role: "group", "aria-label": "Reading direction" }, modeBtns), count);
  const prevCh = el("button", { class: "rd-btn rd-ch", onclick: () => jumpChapter(-1) }, "Previous chapter");
  const nextCh = el("button", { class: "rd-btn rd-ch", onclick: () => jumpChapter(1) }, "Next chapter");
  const slider = el("input", { class: "rd-slider", type: "range", min: "0", max: "0", value: "0", "aria-label": "Page" });
  slider.addEventListener("input", () => goTo(+slider.value, false));
  const bottom = el("div", { class: "rd-bar rd-bottom" }, prevCh, slider, nextCh);
  const stage = el("div", { class: "rd-stage" });
  box.append(stage, top, bottom);
  document.body.append(box);
  document.body.style.overflow = "hidden";
  r = { box, stage, title, count, slider, prevCh, nextCh, modeBtns, onClose, hideT: 0, saveT: 0, io: null, page: 0, zoom: false };
  box.addEventListener("keydown", onKey);
  box.addEventListener("mousemove", showBars);
  box.addEventListener("touchstart", onTouchStart, { passive: true });
  box.addEventListener("touchend", onTouchEnd, { passive: true });
  stage.addEventListener("scroll", onScroll, { passive: true });
  stage.addEventListener("click", onStageClick);
  window.addEventListener("resize", onResize);
  box.focus();
}

function close() {
  if (!r) return;
  saveNow();
  r.io?.disconnect();
  clearTimeout(r.hideT);
  window.removeEventListener("resize", onResize);
  r.box.remove();
  document.body.style.overflow = "";
  const done = r.onClose;
  r = null;
  done?.();
}

/* ---------- a chapter ---------- */

function load(o, root) {
  r.o = o;
  r.url = (i) => `${root}/read/${o.ticket}/${i}`;
  r.title.replaceChildren(el("strong", { text: o.title }), el("span", { text: chName(o.chapter) }));
  r.slider.max = String(Math.max(0, o.pages.length - 1));
  r.prevCh.disabled = o.prev == null;
  r.nextCh.disabled = o.next == null;
  r.zoom = false;
  setMode(o.mode, false, o.page);
  showBars();
}

const chName = (c) => (c.number != null ? `Chapter ${c.number}` : c.name || "");

function setMode(mode, save, page = r.page) {
  r.mode = MODES.some((m) => m.id === mode) ? mode : "manga";
  r.modeBtns.forEach((b) => b.classList.toggle("on", b.dataset.mode === r.mode));
  r.box.dataset.mode = r.mode;
  if (save) invoke("read_mode", { key: r.key, mode: r.mode }).catch(() => {});
  r.mode === "webtoon" ? drawScroll(page) : drawPaged(page);
}

/* ---------- webtoon: one column ---------- */

function drawScroll(page) {
  r.io?.disconnect();
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
  r.cells = imgs;
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
    const target = imgs[Math.min(page, imgs.length - 1)];
    r.stage.scrollTop = target ? target.offsetTop : 0;
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
  const watchEnd = (card) => { seen.observe(card); return card; };
  if (o.next == null) return watchEnd(el("div", { class: "rd-end" }, el("p", { text: "You're up to date." }), el("button", { class: "btn soft", onclick: close }, "Close")));
  const card = el("div", { class: "rd-end" }, el("p", { text: "Next chapter" }), el("button", { class: "btn play", onclick: () => jumpChapter(1) }, "Keep reading"));
  // Rolled into view and left there: the next chapter follows on.
  let t = 0;
  const io = new IntersectionObserver(([s]) => {
    clearTimeout(t);
    if (s.isIntersecting && s.intersectionRatio > 0.6) t = setTimeout(() => { if (r?.o === o) jumpChapter(1); }, 1200);
  }, { root: r.stage, threshold: [0, 0.6, 1] });
  io.observe(card);
  return watchEnd(card);
}

function onScroll() {
  if (!r || r.mode !== "webtoon" || !r.cells) return;
  // The page at the top third of the screen is the one you're on.
  const y = r.stage.scrollTop + r.stage.clientHeight / 3;
  let lo = 0, hi = r.cells.length - 1;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (r.cells[mid].offsetTop <= y) lo = mid; else hi = mid - 1;
  }
  if (lo !== r.page) { r.page = lo; paintCount(); saveSoon(); }
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
  r.cells = null;
  r.spreads = spreads();
  r.stage.replaceChildren(el("div", { class: "rd-paged" }));
  goTo(page, true);
}

function goTo(page, force) {
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
  if (!force && spread.includes(r.page) && r.stage.firstChild?.childElementCount) { r.page = page; paintCount(); return; }
  r.page = spread[0];
  r.zoom = false;
  r.box.classList.remove("zoomed");
  const order = r.mode === "manga" ? spread.slice().reverse() : spread;
  const holder = r.stage.firstChild;
  holder.className = "rd-paged" + (spread.length === 2 ? " two" : "");
  holder.replaceChildren(...order.map((i) => el("img", { src: r.url(i), alt: `Page ${i + 1}`, draggable: "false", decoding: "async" })));
  r.stage.scrollTop = 0;
  r.stage.scrollLeft = 0;
  // The next few, so turning is instant.
  for (const s of r.spreads.slice(at + 1, at + 1 + PRELOAD)) for (const i of s) { const im = new Image(); im.src = r.url(i); }
  paintCount();
  saveSoon();
}

function step(dir) {
  // dir: +1 = onward in the story.
  if (r.mode === "webtoon") { r.stage.scrollBy({ top: dir * r.stage.clientHeight * 0.85, behavior: "smooth" }); return; }
  const at = r.spreads.findIndex((s) => s.includes(r.page));
  const next = at + dir;
  if (next >= r.spreads.length) { if (r.o.next != null) jumpChapter(1); else toast("That's the last chapter for now."); return; }
  if (next < 0) { if (r.o.prev != null) jumpChapter(-1, true); return; }
  goTo(r.spreads[next][0], true);
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

function onStageClick(e) {
  if (r.mode === "webtoon") { toggleBars(); return; }
  const x = e.clientX / window.innerWidth;
  // Manga reads right to left: the left side goes on.
  const onward = r.mode === "manga" ? x < 0.33 : x > 0.67;
  const back = r.mode === "manga" ? x > 0.67 : x < 0.33;
  if (onward) step(1); else if (back) step(-1); else toggleBars();
}

let lastTap = 0;
let touch = null;
function onTouchStart(e) {
  if (e.touches.length !== 1) { touch = null; return; }
  touch = { x: e.touches[0].clientX, y: e.touches[0].clientY, t: Date.now() };
}
function onTouchEnd(e) {
  if (!touch || !r || r.mode === "webtoon") return;
  const t = e.changedTouches[0];
  const dx = t.clientX - touch.x, dy = t.clientY - touch.y;
  const now = Date.now();
  if (now - lastTap < 300 && Math.abs(dx) < 10 && Math.abs(dy) < 10) { zoomToggle(t.clientX, t.clientY); lastTap = 0; return; }
  lastTap = now;
  if (r.zoom || Math.abs(dx) < 50 || Math.abs(dx) < Math.abs(dy) * 1.5) return;
  // A swipe pulls the next page in from where it lies.
  const left = dx < 0;
  step(r.mode === "manga" ? (left ? -1 : 1) : (left ? 1 : -1));
}

function zoomToggle(x, y) {
  r.zoom = !r.zoom;
  r.box.classList.toggle("zoomed", r.zoom);
  if (r.zoom) requestAnimationFrame(() => {
    r.stage.scrollLeft = (x / window.innerWidth) * (r.stage.scrollWidth - r.stage.clientWidth);
    r.stage.scrollTop = (y / window.innerHeight) * (r.stage.scrollHeight - r.stage.clientHeight);
  });
}

function onKey(e) {
  const k = e.key || "";
  const code = e.keyCode;
  const manga = r.mode === "manga";
  if (k === "Escape" || k === "Backspace" || code === 461 /* webOS Back */) { e.preventDefault(); close(); return; }
  if (r.mode === "webtoon") {
    if (k === "ArrowDown" || k === "PageDown" || k === " " || code === 40) { e.preventDefault(); step(1); }
    else if (k === "ArrowUp" || k === "PageUp" || code === 38) { e.preventDefault(); step(-1); }
    else if (k === "ArrowRight" || code === 39) { e.preventDefault(); jumpChapter(1); }
    else if (k === "ArrowLeft" || code === 37) { e.preventDefault(); jumpChapter(-1); }
  } else if (k === "ArrowLeft" || code === 37) { e.preventDefault(); step(manga ? 1 : -1); }
  else if (k === "ArrowRight" || code === 39) { e.preventDefault(); step(manga ? -1 : 1); }
  else if (k === " " || k === "PageDown" || k === "ArrowDown" || code === 40) { e.preventDefault(); step(1); }
  else if (k === "PageUp" || k === "ArrowUp" || code === 38) { e.preventDefault(); step(-1); }
  else if (k === "Enter" || code === 13) { e.preventDefault(); toggleBars(); }
  showBars();
}

let resizeT = 0;
function onResize() {
  clearTimeout(resizeT);
  resizeT = setTimeout(() => { if (r && r.mode !== "webtoon") drawPaged(r.page); }, 150);
}

/* ---------- bars, count, saving ---------- */

function showBars() {
  if (!r) return;
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
  r.count.textContent = s && s.length === 2 ? `${s[0] + 1}-${s[1] + 1} / ${n}` : `${r.page + 1} / ${n}`;
  r.slider.value = String(r.page);
  // Right to left, the slider runs that way too.
  r.slider.classList.toggle("rtl", r.mode === "manga");
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
