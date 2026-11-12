// How Chillarr looks: a style (Cinema, Midnight, Classic or Velvet) and a shade (dark,
// light or custom colours), kept per profile and separately for watching and reading, so
// Movies can be Cinema while reading is Classic. The Look button in the top
// bar opens the picker. styles.css does the rest from body[data-rstyle] and
// body[data-rshade].
import { el, getPref, icon, setPref } from "./api.js";

export const STYLES = [["cinema", "Cinema"], ["midnight", "Midnight"], ["classic", "Classic"], ["velvet", "Velvet"]];

/** "read" while reading, "watch" otherwise (Settings and Explore keep the mode they were opened from). */
const modeNow = () => (document.body.dataset.mode === "read" ? "read" : "watch");

export function styleNow(mode = modeNow()) {
  const s = getPref(`${mode}-style`);
  return STYLES.some(([k]) => k === s) ? s : "cinema";
}

/** Dark, light or custom, kept for each style: Classic starts light, the others dark. */
export function shadeNow(style = styleNow(), mode = modeNow()) {
  const v = getPref(`${mode}-shade-${style}`);
  return v === "light" || v === "dark" || v === "custom" ? v : style === "classic" ? "light" : "dark";
}

/* ---------- Custom colours ----------
   Four picks (page, cards, words, accent) for any style, kept per style and
   mode. Everything else on the page is worked out from them, and the page is
   treated as dark or light by how bright the page pick is, so every rule
   written for a light page still applies. */
const CUSTOM_PICKS = [["bg", "Page"], ["surface", "Cards"], ["text", "Words"], ["accent", "Accent"]];
const VARS = ["--bg", "--bg-rgb", "--bg-2", "--surface", "--surface-hi", "--raised", "--line", "--line-2", "--text", "--muted", "--dim",
  "--accent", "--accent-rgb", "--accent-hi", "--accent-tint", "--accent-2", "--accent-ink", "background", "color"];

const rgbOf = (hex) => { const n = parseInt(String(hex).replace("#", ""), 16) || 0; return [(n >> 16) & 255, (n >> 8) & 255, n & 255]; };
const hexOf = (c) => `#${c.map((v) => Math.round(Math.max(0, Math.min(255, v))).toString(16).padStart(2, "0")).join("")}`;
const mix = (a, b, t) => { const x = rgbOf(a), y = rgbOf(b); return hexOf(x.map((v, i) => v + (y[i] - v) * t)); };
const lum = (hex) => {
  const [r, g, b] = rgbOf(hex).map((v) => { v /= 255; return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
};
const isHex = (v) => /^#[0-9a-f]{6}$/i.test(String(v || "").trim());

function customOf(style = styleNow(), mode = modeNow()) {
  try {
    const c = JSON.parse(getPref(`${mode}-custom-${style}`) || "null");
    if (c && CUSTOM_PICKS.every(([k]) => isHex(c[k]))) return c;
  } catch (e) { /* none saved */ }
  return null;
}

/** The colours showing now, as Custom's starting point. */
function colorsNow() {
  const cs = getComputedStyle(document.body);
  const fall = { bg: "#141414", surface: "#1e1e1e", text: "#f2f2f2", accent: "#ff6a3d" };
  const out = {};
  for (const [k] of CUSTOM_PICKS) { const v = cs.getPropertyValue(`--${k}`).trim(); out[k] = isHex(v) ? v.toLowerCase() : fall[k]; }
  return out;
}

/** Every page variable from the four picks, and whether the page is light. */
export function customVars(c) {
  const light = lum(c.bg) > 0.45;
  const rgb = (h) => rgbOf(h).join(", ");
  const toward = light ? "#000000" : "#ffffff";
  return {
    light,
    vars: {
      "--bg": c.bg, "--bg-rgb": rgb(c.bg), "--bg-2": mix(c.bg, c.surface, 0.4), "--surface": c.surface,
      "--surface-hi": mix(c.surface, c.text, 0.03), "--raised": mix(c.surface, c.text, 0.08),
      "--line": `rgba(${rgb(c.text)}, 0.1)`, "--line-2": `rgba(${rgb(c.text)}, 0.18)`,
      "--text": c.text, "--muted": mix(c.text, c.bg, 0.38), "--dim": mix(c.text, c.bg, 0.58),
      "--accent": c.accent, "--accent-rgb": rgb(c.accent), "--accent-hi": mix(c.accent, toward, 0.12),
      "--accent-tint": mix(c.accent, toward, 0.3), "--accent-2": mix(c.accent, toward, 0.2), "--accent-ink": lum(c.accent) > 0.35 ? "#14100c" : "#ffffff",
      background: c.bg, color: c.text,
    },
  };
}

/** Puts the picks on the page (none: the style's own). Returns "light" or "dark", or null. */
function paintCustom(c) {
  const st = document.body.style;
  if (!c) { for (const k of VARS) st.removeProperty(k); delete document.body.dataset.custom; return null; }
  document.body.dataset.custom = "1";
  const { light, vars } = customVars(c);
  for (const [k, v] of Object.entries(vars)) st.setProperty(k, v);
  return light ? "light" : "dark";
}

export function applyLook() {
  const b = document.body;
  const s = styleNow();
  const want = shadeNow(s);
  // Custom: dark or light by the page colour picked, so light-page rules apply too.
  const sh = paintCustom(want === "custom" ? customOf(s) : null) || (want === "custom" ? "dark" : want);
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

/** Dark, light and custom only recolour, so the page stays as it is and the colours glide across. */
export function setShade(k) {
  const s = styleNow();
  // Custom starts from what's showing.
  if (k === "custom" && !customOf(s)) setPref(`${modeNow()}-custom-${s}`, JSON.stringify(colorsNow()));
  setPref(`${modeNow()}-shade-${s}`, k);
  applyLook();
  drawPickers();
}
export const flipShade = () => setShade(shadeNow() === "light" ? "dark" : "light");

/** Custom's four picks: the page changes while you drag, and is kept when you let go. */
function customEditor() {
  const s = styleNow();
  const c = customOf(s) || colorsNow();
  let frame = 0;
  const live = () => {
    cancelAnimationFrame(frame);
    frame = requestAnimationFrame(() => { const sh = paintCustom(c); if (sh && document.body.dataset.rshade !== sh) document.body.dataset.rshade = sh; });
  };
  const keep = () => setPref(`${modeNow()}-custom-${s}`, JSON.stringify(c));
  return el("div", { class: "lk-shade-custom" },
    CUSTOM_PICKS.map(([k, label]) => {
      const input = el("input", { type: "color", value: c[k], "aria-label": `${label} colour` });
      input.addEventListener("input", () => { c[k] = input.value; live(); });
      input.addEventListener("change", () => { c[k] = input.value; live(); keep(); });
      return el("label", {}, label, input);
    }),
    el("button", { class: "lk-reset", onclick: () => {
      // The style's own colours again, as a fresh start for Custom.
      paintCustom(null);
      document.body.dataset.rshade = s === "classic" ? "light" : "dark";
      setPref(`${modeNow()}-custom-${s}`, JSON.stringify(colorsNow()));
      applyLook();
      drawPop();
    } }, "Start again from this style"));
}

const SUN = "M12 4V2M12 22v-2M4 12H2M22 12h-2M5.6 5.6 4.2 4.2M19.8 19.8l-1.4-1.4M5.6 18.4l-1.4 1.4M19.8 4.2l-1.4 1.4M12 7a5 5 0 1 0 0 10 5 5 0 0 0 0-10z";
const MOON = "M20 14.5A8 8 0 1 1 9.5 4a6.5 6.5 0 0 0 10.5 10.5z";

// One Look button in the top bar (both modes, every width) opens a small
// menu: the three styles, each with a little picture of itself, and Dark or
// Light. It used to be a segmented control squeezed into the top bar (which
// pushed "My downloads" onto two lines) or a STYLE row on top of every page.
const LOOK = "M12 3a9 9 0 1 0 0 18c1 0 1.5-.6 1.5-1.4 0-.4-.2-.8-.4-1.1-.3-.3-.4-.7-.4-1.1 0-.9.7-1.6 1.6-1.6H16a5 5 0 0 0 5-5c0-4.4-4-7.8-9-7.8zM7.5 12.5a1 1 0 1 0 0-2 1 1 0 0 0 0 2zM10.5 8.5a1 1 0 1 0 0-2 1 1 0 0 0 0 2zM15 8.5a1 1 0 1 0 0-2 1 1 0 0 0 0 2z";
const NOTE = { cinema: "Big pictures and rows", midnight: "Dark, dense, a ranking", classic: "Light, charts and days", velvet: "Burgundy and brass, like a theatre" };
const PALETTE = "M12 3a9 9 0 1 0 0 18c1 0 1.5-.6 1.5-1.4 0-.4-.2-.8-.4-1.1-.3-.3-.4-.7-.4-1.1 0-.9.7-1.6 1.6-1.6H16a5 5 0 0 0 5-5c0-4.4-4-7.8-9-7.8z";
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
  const shade = shadeNow(now);
  pop.replaceChildren(
    el("p", { class: "lk-h", text: document.body.dataset.mode === "read" ? "How reading looks" : "How watching looks" }),
    el("div", { class: "lk-styles" }, STYLES.map(([k, label]) => el("button", { class: `lk-style${k === now ? " on" : ""}`, "aria-pressed": String(k === now), onclick: () => setStyle(k) },
      el("span", { class: `lk-prev lk-${k}`, "aria-hidden": "true" }, el("i"), el("i"), el("i")),
      el("span", { class: "lk-name" }, el("b", { text: label }), el("small", { text: NOTE[k] }))))),
    el("div", { class: "lk-shade", role: "group", "aria-label": "Shade" },
      [["dark", "Dark", MOON], ["light", "Light", SUN], ["custom", "Custom", PALETTE]].map(([k, label, path]) => el("button", { class: shade === k ? "on" : "", "aria-pressed": String(shade === k), onclick: () => { if (shade !== k) setShade(k); } }, icon(path), label))),
    shade === "custom" ? customEditor() : null);
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
