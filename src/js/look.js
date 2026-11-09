// How Chillarr looks: a style (Cinema, Midnight or Classic) and a shade (dark
// or light), kept per profile and separately for watching and reading, so
// Movies can be Cinema while reading is Classic. The Look button in the top
// bar opens the picker. styles.css does the rest from body[data-rstyle] and
// body[data-rshade].
import { el, getPref, icon, setPref } from "./api.js";

export const STYLES = [["cinema", "Cinema"], ["midnight", "Midnight"], ["classic", "Classic"]];

/** "read" while reading, "watch" otherwise (Settings and Explore keep the mode they were opened from). */
const modeNow = () => (document.body.dataset.mode === "read" ? "read" : "watch");

export function styleNow(mode = modeNow()) {
  const s = getPref(`${mode}-style`);
  return STYLES.some(([k]) => k === s) ? s : "cinema";
}

/** Dark or light, kept for each style: Classic starts light, the others dark. */
export function shadeNow(style = styleNow(), mode = modeNow()) {
  const v = getPref(`${mode}-shade-${style}`);
  return v === "light" || v === "dark" ? v : style === "classic" ? "light" : "dark";
}

export function applyLook() {
  const b = document.body;
  const s = styleNow(), sh = shadeNow(s);
  if (b.dataset.rstyle !== s) b.dataset.rstyle = s;
  if (b.dataset.rshade !== sh) b.dataset.rshade = sh;
}

// What each mode does when its style changes (draws its page again).
const redraw = {};
export const onStyle = (mode, fn) => { redraw[mode] = fn; };

const still = () => matchMedia("(prefers-reduced-motion: reduce)").matches;

/** The page fades out, the look changes, the page is drawn in the new style and fades in. */
function swap(change) {
  const v = document.getElementById("view");
  const mode = modeNow();
  if (!v || still()) { change(); redraw[mode]?.(); return; }
  v.classList.add("rs-swap");
  setTimeout(() => {
    change();
    Promise.resolve(redraw[mode]?.()).catch(() => {}).then(() => requestAnimationFrame(() => v.classList.remove("rs-swap")));
    // Never left faded if the page takes long.
    setTimeout(() => v.classList.remove("rs-swap"), 900);
  }, 180);
}

export function setStyle(k) {
  if (k === styleNow()) return;
  setPref(`${modeNow()}-style`, k);
  // The menu shows the pick at once; the page follows.
  drawPickers();
  swap(applyLook);
}

/** Dark and light only recolour, so the page stays as it is and the colours glide across. */
export function flipShade() {
  const s = styleNow();
  setPref(`${modeNow()}-shade-${s}`, shadeNow(s) === "light" ? "dark" : "light");
  applyLook();
  drawPickers();
}

const SUN = "M12 4V2M12 22v-2M4 12H2M22 12h-2M5.6 5.6 4.2 4.2M19.8 19.8l-1.4-1.4M5.6 18.4l-1.4 1.4M19.8 4.2l-1.4 1.4M12 7a5 5 0 1 0 0 10 5 5 0 0 0 0-10z";
const MOON = "M20 14.5A8 8 0 1 1 9.5 4a6.5 6.5 0 0 0 10.5 10.5z";

// One Look button in the top bar (both modes, every width) opens a small
// menu: the three styles, each with a little picture of itself, and Dark or
// Light. It used to be a segmented control squeezed into the top bar (which
// pushed "My downloads" onto two lines) or a STYLE row on top of every page.
const LOOK = "M12 3a9 9 0 1 0 0 18c1 0 1.5-.6 1.5-1.4 0-.4-.2-.8-.4-1.1-.3-.3-.4-.7-.4-1.1 0-.9.7-1.6 1.6-1.6H16a5 5 0 0 0 5-5c0-4.4-4-7.8-9-7.8zM7.5 12.5a1 1 0 1 0 0-2 1 1 0 0 0 0 2zM10.5 8.5a1 1 0 1 0 0-2 1 1 0 0 0 0 2zM15 8.5a1 1 0 1 0 0-2 1 1 0 0 0 0 2z";
const NOTE = { cinema: "Big pictures and rows", midnight: "Dark, dense, a ranking", classic: "Light, charts and days" };
let lookBtn = null;
let pop = null;

function closePop() {
  if (!pop) return;
  const p = pop;
  pop = null;
  lookBtn?.setAttribute("aria-expanded", "false");
  p.classList.remove("in");
  setTimeout(() => p.remove(), 180);
}

function drawPop() {
  if (!pop) return;
  const now = styleNow();
  const light = shadeNow(now) === "light";
  pop.replaceChildren(
    el("p", { class: "lk-h", text: document.body.dataset.mode === "read" ? "How reading looks" : "How watching looks" }),
    el("div", { class: "lk-styles" }, STYLES.map(([k, label]) => el("button", { class: `lk-style${k === now ? " on" : ""}`, "aria-pressed": String(k === now), onclick: () => setStyle(k) },
      el("span", { class: `lk-prev lk-${k}`, "aria-hidden": "true" }, el("i"), el("i"), el("i")),
      el("span", { class: "lk-name" }, el("b", { text: label }), el("small", { text: NOTE[k] }))))),
    el("div", { class: "lk-shade", role: "group", "aria-label": "Shade" },
      [["dark", "Dark", MOON], ["light", "Light", SUN]].map(([k, label, path]) => el("button", { class: (light ? "light" : "dark") === k ? "on" : "", "aria-pressed": String((light ? "light" : "dark") === k), onclick: () => { if ((light ? "light" : "dark") !== k) flipShade(); } }, icon(path), label))));
}

function openPop() {
  if (pop) { closePop(); return; }
  pop = el("div", { class: "look-pop", role: "dialog", "aria-label": "Look" });
  document.body.append(pop);
  const r = lookBtn.getBoundingClientRect();
  // Under the button, kept on screen however narrow it is.
  const w = Math.min(300, innerWidth - 24);
  pop.style.width = `${w}px`;
  pop.style.top = `${Math.round(r.bottom + 10)}px`;
  pop.style.left = `${Math.round(Math.min(Math.max(12, r.right + 8 - w), innerWidth - 12 - w))}px`;
  drawPop();
  lookBtn.setAttribute("aria-expanded", "true");
  requestAnimationFrame(() => pop && pop.classList.add("in"));
  pop.querySelector(".lk-style.on")?.focus();
}

document.addEventListener("pointerdown", (e) => { if (pop && !pop.contains(e.target) && !lookBtn.contains(e.target)) closePop(); }, true);
document.addEventListener("keydown", (e) => { if (e.key === "Escape" && pop) { closePop(); lookBtn?.focus(); } });
window.addEventListener("hashchange", closePop);
window.addEventListener("resize", closePop);

/** The top bar's Look button (made once) and its menu, showing what's picked. */
export function drawPickers() {
  if (!lookBtn) {
    const actions = document.querySelector("#topbar .top-actions");
    if (!actions) return;
    lookBtn = el("button", { class: "icon-btn look-btn", "aria-label": "Look", title: "Look: style, dark or light", "aria-haspopup": "dialog", "aria-expanded": "false", onclick: openPop }, icon(LOOK));
    actions.insertBefore(lookBtn, actions.querySelector("#mode-btn"));
  }
  drawPop();
}

/** The old STYLE row on top of pages: the Look button does it now. Kept so callers needn't change. */
export function inlinePicker() {
  return null;
}
