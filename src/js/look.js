// How Chillarr looks: a style (Cinema, Midnight or Classic) and a shade (dark
// or light), kept per profile and separately for watching and reading, so
// Movies can be Cinema while reading is Classic. The picker sits in the middle
// of the top bar (on a narrow screen, at the top of Home). styles.css does the
// rest from body[data-rstyle] and body[data-rshade].
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
  // The pill moves at once; the page follows.
  drawPickers(k);
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

function styleButtons(now) {
  return STYLES.map(([k, label]) => el("button", { class: k === now ? "on" : "", dataset: { style: k }, "aria-pressed": String(k === now), onclick: () => setStyle(k) }, label));
}

let topPicker = null;
/** The top bar's picker, and any narrow-screen one on the page, showing what's picked. */
export function drawPickers(now = styleNow()) {
  if (!topPicker) {
    const bar = document.getElementById("topbar");
    const actions = bar && bar.querySelector(".top-actions");
    if (!actions) return;
    topPicker = el("div", { class: "read-style", role: "group", "aria-label": "Style" }, el("span", { class: "read-style-label", text: "Style" }), el("div", { class: "read-style-seg" }));
    bar.insertBefore(topPicker, actions);
  }
  const seg = topPicker.querySelector(".read-style-seg");
  const pill = seg.querySelector(".read-style-pill") || el("i", { class: "read-style-pill", "aria-hidden": "true" });
  seg.replaceChildren(...styleButtons(now), pill);
  const light = shadeNow(now) === "light";
  const shade = el("button", { class: "read-shade", "aria-label": light ? "Dark" : "Light", title: light ? "Switch to dark" : "Switch to light", onclick: flipShade }, icon(light ? MOON : SUN));
  const old = topPicker.querySelector(".read-shade");
  if (old) old.replaceWith(shade); else topPicker.append(shade);
  // The pill slides under the picked one (kept, so it glides rather than jumps).
  const place = () => {
    const on = seg.querySelector("button.on");
    if (on && on.offsetWidth) { pill.style.width = `${on.offsetWidth}px`; pill.style.transform = `translateX(${on.offsetLeft - 3}px)`; }
  };
  requestAnimationFrame(place);
  for (const s of document.querySelectorAll(".read-style-inline .chips")) {
    s.replaceChildren(...styleButtons(now).map((b) => { b.className = "chip" + (b.className ? " on" : ""); return b; }),
      el("button", { class: "chip read-shade-chip", "aria-label": light ? "Switch to dark" : "Switch to light", onclick: flipShade }, icon(light ? MOON : SUN), light ? "Dark" : "Light"));
  }
}

/** The style picker at the top of a page, for screens too narrow for the top bar's. */
export function inlinePicker() {
  const p = el("div", { class: "read-style-inline" }, el("span", { text: "Style" }), el("div", { class: "chips" }));
  requestAnimationFrame(() => drawPickers());
  return p;
}
