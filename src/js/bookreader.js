// The book reader: EPUBs from the PC (read.rs serves each file inside the
// book through the bridge). A chapter is drawn in a sandboxed frame with no
// scripts allowed; the book's own styles are folded in (the window's rules
// only let pictures load from the bridge), its pictures load as they are.
// Turning a page scrolls by a screen; past the end, the next chapter.
// PDFs open in the browser's own viewer; other formats are saved instead.
// Comics (CBZ) open in the comic reader (reader.js), page by page.
import { el, getPref, icon, ICON, invoke, setPref, toast, WEB } from "./api.js";
import { openComic } from "./reader.js";

const THEMES = { dark: ["#111214", "#e4e4e6"], sepia: ["#f4ecd8", "#3b3024"], light: ["#ffffff", "#1a1a1a"] };
let base = null;
let b = null;

async function bridgeBase() {
  if (!base) base = WEB ? await window.__CHILLARR_BRIDGE__() : await invoke("read_base");
  return base;
}

/** Opens a book where you stopped, or from its first page with `fromStart` (Read again). */
export async function openBook(key, onClose = null, fromStart = false) {
  let o;
  let root;
  try { [o, root] = await Promise.all([invoke("read_book_open", { key }), bridgeBase()]); }
  catch (e) { base = null; toast(String(e), true); return; }
  const file = `${root}/read/${o.ticket}/file`;
  if (o.kind === "pdf") { window.open(file, "_blank"); return; }
  if (fromStart) { o.chapter = 0; o.at = 0; o.page = 0; }
  // A comic (CBZ): its pages in the comic reader.
  if (o.kind === "comic") { openComic(key, o, root, onClose); return; }
  if (o.kind !== "epub") {
    // AZW3 / MOBI: nothing here shows them; the file itself is offered.
    const a = el("a", { href: file, download: "" });
    document.body.append(a); a.click(); a.remove();
    toast("This book's format only opens in an e-reader app. It's being saved.");
    return;
  }
  build(key, o, root, onClose);
}

function build(key, o, root, onClose) {
  const frame = el("iframe", { class: "bk-frame", sandbox: "allow-same-origin", title: o.title });
  const title = el("div", { class: "rd-title" }, el("strong", { text: o.title }), el("span", { class: "bk-where" }));
  const smaller = el("button", { class: "rd-btn", "aria-label": "Smaller text", onclick: () => font(-1) }, "A-");
  const bigger = el("button", { class: "rd-btn", "aria-label": "Bigger text", onclick: () => font(1) }, "A+");
  const themeBtns = Object.keys(THEMES).map((t) => el("button", { class: "rd-mode", dataset: { theme: t }, onclick: () => theme(t) }, t[0].toUpperCase() + t.slice(1)));
  const top = el("div", { class: "rd-bar rd-top" },
    el("button", { class: "rd-btn", "aria-label": "Close", onclick: close }, icon(ICON.left)), title,
    el("div", { class: "rd-modes" }, themeBtns), smaller, bigger);
  const slider = el("input", { class: "rd-slider", type: "range", min: "0", max: String(o.spine.length - 1), value: String(o.chapter), "aria-label": "Chapter" });
  slider.addEventListener("change", () => show(+slider.value, 0));
  const bottom = el("div", { class: "rd-bar rd-bottom" },
    el("button", { class: "rd-btn rd-ch", onclick: () => turn(-1) }, "Back"), slider, el("button", { class: "rd-btn rd-ch", onclick: () => turn(1) }, "Next"));
  const box = el("div", { class: "reader book", tabindex: "-1", role: "dialog", "aria-label": o.title }, el("div", { class: "rd-stage bk-stage" }, frame), top, bottom);
  document.body.append(box);
  document.body.style.overflow = "hidden";
  b = { key, o, root, box, frame, slider, themeBtns, where: title.lastChild, onClose, idx: o.chapter, saveT: 0, hideT: 0,
    size: +getPref("book-size") || 19, theme: getPref("book-theme") || "dark" };
  box.addEventListener("keydown", onKey);
  box.addEventListener("mousemove", bars);
  frame.addEventListener("load", wire);
  box.focus();
  theme(b.theme, false);
  show(o.chapter, o.at / 1000);
  bars();
}

function close() {
  if (!b) return;
  save();
  b.box.remove();
  document.body.style.overflow = "";
  const done = b.onClose;
  b = null;
  done?.();
}

/* ---------- a chapter ---------- */

const dirOf = (p) => (p.includes("/") ? p.slice(0, p.lastIndexOf("/") + 1) : "");
const fileUrl = (p) => `${b.root}/read/${b.o.ticket}/f/${p.split("/").map(encodeURIComponent).join("/")}`;

/** `href` (relative to `from`) as a path inside the book. */
function resolve(from, href) {
  const parts = (dirOf(from) + href.split("#")[0]).split("/");
  const out = [];
  for (const p of parts) { if (p === "..") out.pop(); else if (p && p !== ".") out.push(p); }
  try { return decodeURIComponent(out.join("/")); } catch (e) { return out.join("/"); }
}

async function show(idx, at) {
  if (!b) return;
  idx = Math.max(0, Math.min(idx, b.o.spine.length - 1));
  const path = b.o.spine[idx];
  let text;
  // An error page (an expired link, the PC busy) isn't the chapter: what's showing stays.
  try { const res = await fetch(fileUrl(path)); if (!res.ok) throw new Error(String(res.status)); text = await res.text(); } catch (e) { toast("Couldn't open that part of the book. Try again in a moment.", true); return; }
  if (!b) return;
  b.idx = idx;
  b.pendingAt = at;
  b.slider.value = String(idx);
  b.where.textContent = `Part ${idx + 1} of ${b.o.spine.length}`;
  b.frame.srcdoc = await clean(text, path);
}

/** The chapter, safe to show: no scripts, no frames, its styles folded in. */
async function clean(text, path) {
  let doc = new DOMParser().parseFromString(text, "application/xhtml+xml");
  if (doc.querySelector("parsererror")) doc = new DOMParser().parseFromString(text, "text/html");
  doc.querySelectorAll("script, iframe, object, embed, form, link[rel=import], meta[http-equiv]").forEach((n) => n.remove());
  doc.querySelectorAll("*").forEach((n) => {
    for (const a of Array.from(n.attributes)) {
      if (/^on/i.test(a.name) || /^\s*javascript:/i.test(a.value)) n.removeAttribute(a.name);
    }
  });
  const styles = [];
  for (const l of Array.from(doc.querySelectorAll("link[rel~=stylesheet][href]"))) {
    const css = resolve(path, l.getAttribute("href"));
    try {
      const res = await fetch(fileUrl(css));
      if (!res.ok) continue;
      const t = await res.text();
      // url(...) inside the sheet is relative to the sheet.
      styles.push(t.replace(/url\(\s*(['"]?)(?!data:|https?:)([^'")]+)\1\s*\)/g, (_, q, u) => `url("${fileUrl(resolve(css, u))}")`));
    } catch (e) { /* a missing sheet just isn't used */ }
    l.remove();
  }
  // Pictures: SVG <image> and <img> point at the bridge.
  doc.querySelectorAll("img[src]").forEach((i) => i.setAttribute("src", fileUrl(resolve(path, i.getAttribute("src")))));
  doc.querySelectorAll("image").forEach((i) => {
    const h = i.getAttribute("xlink:href") || i.getAttribute("href");
    if (h) { i.setAttribute("href", fileUrl(resolve(path, h))); i.setAttribute("xlink:href", fileUrl(resolve(path, h))); }
  });
  const [bg, fg] = THEMES[b.theme];
  const own = `html{background:${bg}}body{background:${bg}!important;color:${fg}!important;max-width:40em;margin:0 auto!important;padding:9vh 6vw 14vh!important;font-size:${b.size}px!important;line-height:1.65;font-family:Georgia,"Times New Roman",serif;word-wrap:break-word}
    *{color:inherit!important;background-color:transparent!important}body{background:${bg}!important}a{text-decoration:underline}
    img,svg{max-width:100%!important;height:auto!important}p{margin:0 0 .9em}h1,h2,h3{line-height:1.25}`;
  const body = doc.body ? doc.body.innerHTML : doc.documentElement.innerHTML;
  return `<!doctype html><html><head><meta charset="utf-8"><style>${styles.join("\n")}</style><style>${own}</style></head><body>${body}</body></html>`;
}

/** Once a chapter is drawn: where you were, clicks, keys and links. */
function wire() {
  if (!b) return;
  const w = b.frame.contentWindow;
  const d = b.frame.contentDocument;
  if (!w || !d) return;
  const at = b.pendingAt;
  const go = () => w.scrollTo(0, at === "end" ? d.documentElement.scrollHeight : (at || 0) * (d.documentElement.scrollHeight - w.innerHeight));
  go();
  // Pictures arriving later move the text: once more after they load.
  setTimeout(go, 400);
  w.addEventListener("scroll", () => { saveSoon(); }, { passive: true });
  d.addEventListener("keydown", onKey);
  d.addEventListener("mousemove", bars);
  d.addEventListener("click", (e) => {
    const a = e.target.closest && e.target.closest("a[href]");
    if (a) {
      e.preventDefault();
      const href = a.getAttribute("href");
      if (/^https?:/i.test(href)) return;
      const to = resolve(b.o.spine[b.idx], href);
      const i = b.o.spine.findIndex((p) => p === to);
      if (i >= 0) show(i, 0);
      return;
    }
    const x = e.clientX / w.innerWidth;
    if (x > 0.7) turn(1); else if (x < 0.3) turn(-1); else toggleBars();
  });
}

function turn(dir) {
  const w = b.frame.contentWindow;
  const d = b.frame.contentDocument;
  if (!w || !d) return;
  const max = d.documentElement.scrollHeight - w.innerHeight;
  const y = w.scrollY;
  if (dir > 0 && y >= max - 4) {
    if (b.idx + 1 < b.o.spine.length) show(b.idx + 1, 0);
    else { save(true); toast("That's the end of the book."); }
    return;
  }
  if (dir < 0 && y <= 4) { if (b.idx > 0) show(b.idx - 1, "end"); return; }
  w.scrollBy({ top: dir * (w.innerHeight * 0.9), behavior: "smooth" });
}

function onKey(e) {
  if (!b) return;
  const k = e.key || "";
  if (k === "Escape" || k === "Backspace" || e.keyCode === 461) { e.preventDefault(); close(); return; }
  if (["ArrowRight", "ArrowDown", "PageDown", " "].includes(k)) { e.preventDefault(); turn(1); }
  else if (["ArrowLeft", "ArrowUp", "PageUp"].includes(k)) { e.preventDefault(); turn(-1); }
  bars();
}

function font(step) {
  b.size = Math.max(14, Math.min(30, b.size + step));
  setPref("book-size", String(b.size));
  rerender();
}

function theme(t, redraw = true) {
  b.theme = THEMES[t] ? t : "dark";
  setPref("book-theme", b.theme);
  b.themeBtns.forEach((x) => x.classList.toggle("on", x.dataset.theme === b.theme));
  b.box.dataset.theme = b.theme;
  if (redraw) rerender();
}

function rerender() {
  const w = b.frame.contentWindow, d = b.frame.contentDocument;
  const at = w && d ? w.scrollY / Math.max(1, d.documentElement.scrollHeight - w.innerHeight) : 0;
  show(b.idx, at);
}

function bars() {
  if (!b) return;
  b.box.classList.add("bars");
  clearTimeout(b.hideT);
  b.hideT = setTimeout(() => b?.box.classList.remove("bars"), 2800);
}
function toggleBars() { if (b.box.classList.contains("bars")) b.box.classList.remove("bars"); else bars(); }

function saveSoon() { clearTimeout(b.saveT); b.saveT = setTimeout(() => save(), 1000); }

function save(end = false) {
  if (!b) return;
  const w = b.frame.contentWindow, d = b.frame.contentDocument;
  let at = 0;
  if (w && d) at = Math.round((w.scrollY / Math.max(1, d.documentElement.scrollHeight - w.innerHeight)) * 1000);
  if (end) at = 1000;
  // chapter = spine part, page = how far into it (0..1000), pages = how many parts.
  invoke("read_progress", { key: b.key, chapter: b.idx, page: Math.max(0, Math.min(1000, at)), pages: b.o.spine.length }).catch(() => {});
}
