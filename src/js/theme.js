// Colours and the logo. Every colour on screen comes from a few variables set
// here, worked out from one accent, so any colour someone picks looks finished.
// The logo is redrawn in the same colours for the window, tray and shortcuts.
import { el, invoke, WEB } from "./api.js";

export const THEMES = [
  { id: "ember", name: "Ember", accent: "#ff6a3d" },
  { id: "tide", name: "Tide", accent: "#3fb8ff", tint: [212, 34] },
  { id: "orchid", name: "Orchid", accent: "#c08bff", tint: [268, 26] },
  { id: "moss", name: "Moss", accent: "#4fd08a", tint: [155, 20] },
  { id: "marquee", name: "Marquee", accent: "#ff3d5a", tint: [350, 18] },
  { id: "reel", name: "Reel", accent: "#f2c14e", tint: [38, 16] },
  { id: "mono", name: "Mono", accent: "#e7e7ec", tint: [240, 0] },
  // Soft cherry-blossom pink over a plum-tinted dark.
  { id: "sakura", name: "Sakura", accent: "#ff8fb8", tint: [325, 24] },
  // Teal over deep indigo: the one preset whose surfaces aren't its accent's hue.
  { id: "aurora", name: "Aurora", accent: "#4fe3c1", tint: [248, 34] },
];
const DEFAULT = { preset: "ember", accent: "#ff6a3d", surface: "tinted" };
const KEY = "chillarr-theme";

function hexToRgb(hex) {
  const n = parseInt(hex.slice(1), 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}
function rgbToHsl([r, g, b]) {
  r /= 255; g /= 255; b /= 255;
  const max = Math.max(r, g, b), min = Math.min(r, g, b), d = max - min;
  const l = (max + min) / 2;
  if (!d) return [0, 0, l * 100];
  const s = d / (1 - Math.abs(2 * l - 1));
  const h = max === r ? ((g - b) / d + 6) % 6 : max === g ? (b - r) / d + 2 : (r - g) / d + 4;
  return [h * 60, s * 100, l * 100];
}
function hslToRgb(h, s, l) {
  h = ((h % 360) + 360) % 360; s /= 100; l /= 100;
  const c = (1 - Math.abs(2 * l - 1)) * s, x = c * (1 - Math.abs(((h / 60) % 2) - 1)), m = l - c / 2;
  const [r, g, b] = h < 60 ? [c, x, 0] : h < 120 ? [x, c, 0] : h < 180 ? [0, c, x] : h < 240 ? [0, x, c] : h < 300 ? [x, 0, c] : [c, 0, x];
  return [r, g, b].map((v) => Math.round((v + m) * 255));
}
const rgbToHex = (rgb) => `#${rgb.map((v) => v.toString(16).padStart(2, "0")).join("")}`;
const hsl = (h, s, l) => rgbToHex(hslToRgb(h, s, l));

/** How bright a colour looks (WCAG), 0 to 1. */
function luminance(rgb) {
  const [r, g, b] = rgb.map((v) => { v /= 255; return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

/** Keeps a picked colour readable on a dark page: a near-black pick is lifted until it shows. */
export function usableAccent(hex) {
  const [h, s, l] = rgbToHsl(hexToRgb(hex));
  return l >= 48 && l <= 86 ? hex : hsl(h, s, Math.min(86, Math.max(48, l)));
}

/** Every variable for a theme. Ember keeps its exact original values. */
export function palette(theme) {
  const accent = usableAccent(theme.accent);
  const rgb = hexToRgb(accent);
  const [h, s, l] = rgbToHsl(rgb);
  const preset = THEMES.find((t) => t.id === theme.preset);
  const isEmber = theme.preset === "ember" && accent === DEFAULT.accent;
  const [th, ts] = preset?.tint || (isEmber ? [230, 18] : [h, Math.min(24, s * 0.3)]);
  const black = theme.surface === "black";
  const bg = black ? [0, 0, 0] : isEmber ? [11, 12, 16] : hslToRgb(th, ts, 5);
  const shade = (light) => (black ? hsl(th, Math.min(ts, 8), light - 2) : hsl(th, ts, light));
  return {
    accent,
    "accent-rgb": rgb.join(", "),
    "accent-hi": isEmber ? "#ff7d55" : hsl(h, s, Math.min(92, l + 7)),
    "accent-2": isEmber ? "#ffb24a" : hsl(h + 32, Math.min(100, s + 6), Math.min(80, l + 8)),
    "accent-tint": isEmber ? "#ffd2c2" : hsl(h, Math.min(100, s), Math.min(92, l + 24)),
    "accent-ink": luminance(rgb) > 0.32 ? hsl(h, Math.min(70, s), 6) : "#ffffff",
    bg: rgbToHex(bg),
    "bg-rgb": bg.join(", "),
    "bg-2": isEmber && !black ? "#101217" : shade(7.5),
    surface: isEmber && !black ? "#15171d" : shade(10),
    "surface-hi": isEmber && !black ? "#181a21" : shade(12),
    raised: isEmber && !black ? "#1d2028" : shade(14),
  };
}

let current = { ...DEFAULT };
export const currentTheme = () => current;

/* ---------- Each tab's own light ----------
   Away from Home every tab has a glow of its own at the top of the page, a
   turn of the theme's accent around the colour wheel, so it works with any
   theme (Mono stays grey). Only two CSS variables change on a tab switch;
   the glow itself is one fixed, still layer (styles.css body::before). */
const TAB_TURN = {
  movies: 0, series: 48, anime: -48, mylist: 150, downloads: 96, explore: 0, filter: 0, maker: 24,
  "r-home": 200, "r-manga": -24, "r-manhwa": 28, "r-manhua": 62, "r-novels": 118, "r-books": 210,
  "r-mylist": 150, "r-downloads": 96, "r-all": 0, "r-author": 210, "r-maker": 24,
};
let tabNow = "home";
export function setTabTint(tab) {
  tabNow = tab;
  const root = document.documentElement.style;
  const turn = TAB_TURN[tab];
  if (turn === undefined) { root.setProperty("--tab-a", "0"); root.setProperty("--tab-a2", "0"); return; }
  const [h, s] = rgbToHsl(hexToRgb(usableAccent(current.accent)));
  const sat = Math.min(80, s);
  root.setProperty("--tab-rgb", hslToRgb(h + turn, sat, 58).join(", "));
  root.setProperty("--tab-rgb2", hslToRgb(h + turn + 36, sat, 52).join(", "));
  root.setProperty("--tab-a", "0.26");
  root.setProperty("--tab-a2", "0.16");
}

/** The colours, at once. Nothing is saved. */
export function applyTheme(theme) {
  current = { ...DEFAULT, ...theme };
  const vars = palette(current);
  const root = document.documentElement.style;
  for (const k of Object.keys(vars)) root.setProperty(`--${k}`, vars[k]);
  setTabTint(tabNow);
  document.querySelector('meta[name="theme-color"]')?.setAttribute("content", vars.bg);
  try { localStorage.setItem(KEY, JSON.stringify(current)); } catch (e) { /* storage blocked */ }
  drawLogos();
}

/* ---------- The logo ---------- */
let logoText = null;
const isDefaultLogo = () => usableAccent(current.accent) === DEFAULT.accent;

async function logoSvg() {
  if (logoText === null) logoText = await fetch("logo.svg").then((r) => r.text()).catch(() => "");
  if (!logoText || isDefaultLogo()) return logoText;
  const [h, s, l] = rgbToHsl(hexToRgb(usableAccent(current.accent)));
  const swap = { "#ff7a4a": hsl(h, s, Math.min(88, l + 6)), "#f2552c": hsl(h, s, Math.max(38, l - 9)), "#ffd25a": hsl(h + 32, Math.min(100, s + 6), Math.min(86, l + 12)), "#ffb224": hsl(h + 32, Math.min(100, s + 6), Math.min(78, l + 2)) };
  return Object.keys(swap).reduce((t, k) => t.split(k).join(swap[k]), logoText);
}

const toData = (svg) => `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`;
let logoSeq = 0;

/** The logo on the page and the tab icon, in the theme's colours. */
async function drawLogos() {
  const seq = ++logoSeq;
  const svg = await logoSvg();
  if (seq !== logoSeq || !svg) return;
  const src = isDefaultLogo() ? "logo.svg" : toData(svg);
  document.querySelectorAll("img.theme-logo, .brand img").forEach((i) => { if (i.getAttribute("src") !== src) i.src = src; });
  document.querySelector('link[rel="icon"]')?.setAttribute("href", src);
}

export function logoImg(cls) {
  const i = el("img", { class: `theme-logo ${cls || ""}`, alt: "", src: "logo.svg" });
  drawLogos();
  return i;
}

function canvasOf(img, side) {
  const c = document.createElement("canvas");
  c.width = c.height = side;
  const ctx = c.getContext("2d");
  ctx.imageSmoothingQuality = "high";
  ctx.drawImage(img, 0, 0, side, side);
  return c;
}

const b64 = (bytes) => {
  let s = "";
  for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000));
  return btoa(s);
};

/** Hands the logo in this theme's colours to the window, tray and shortcuts. */
async function pushIcon() {
  if (WEB) return;
  const accent = usableAccent(current.accent);
  if (isDefaultLogo()) return invoke("theme_icon", { accent, pngs: [], rgba: null, side: 0 });
  const img = new Image();
  await new Promise((done, fail) => { img.onload = done; img.onerror = fail; logoSvg().then((svg) => { img.src = toData(svg); }); });
  const pngs = [];
  for (const side of [16, 24, 32, 48, 64, 256]) {
    const blob = await new Promise((done) => canvasOf(img, side).toBlob(done, "image/png"));
    pngs.push([side, b64(new Uint8Array(await blob.arrayBuffer()))]);
  }
  const small = canvasOf(img, 64).getContext("2d").getImageData(0, 0, 64, 64).data;
  return invoke("theme_icon", { accent, pngs, rgba: b64(new Uint8Array(small.buffer)), side: 64 });
}

/** Keeps the choice and changes the icons. */
export async function saveTheme(theme) {
  applyTheme(theme);
  if (WEB) return;
  await invoke("theme_save", { theme: current });
  await pushIcon();
}

/** At start: Windows' icons follow the saved theme (a reinstall resets the shortcuts). */
export function syncIcons() {
  pushIcon().catch(() => {});
}

/* ---------- The picker ---------- */
/** Colour dots plus "your own colour". Changes as you drag, saves when you let go. */
export function themePicker(onSaved) {
  const box = el("div", { class: "theme-pick", role: "group", "aria-label": "Colour" });
  const custom = el("input", { type: "color", "aria-label": "Pick your own colour", value: current.preset === "custom" ? current.accent : "#7c8cff" });
  const customDot = el("label", { class: "theme-dot custom", title: "Your own colour" }, custom, el("span", { class: "dot-fill" }), el("span", { text: "Yours" }));
  const dots = THEMES.map((t) => el("button", {
    class: "theme-dot", type: "button", title: t.name, "aria-label": t.name, dataset: { id: t.id },
    onclick: async () => { await saveTheme({ ...current, preset: t.id, accent: t.accent }); draw(); onSaved?.(); },
  }, el("span", { class: "dot-fill", style: `background:${t.accent}` }), el("span", { text: t.name })));
  const draw = () => {
    for (const d of dots) {
      const on = current.preset === d.dataset.id;
      d.classList.toggle("on", on);
      d.setAttribute("aria-pressed", on ? "true" : "false");
    }
    customDot.classList.toggle("on", current.preset === "custom");
    customDot.querySelector(".dot-fill").style.background = current.preset === "custom" ? current.accent : "conic-gradient(#ff5f6d, #ffc371, #4fd08a, #3fb8ff, #c08bff, #ff5f6d)";
  };
  // Live while dragging: only CSS variables change, which costs one style pass.
  custom.addEventListener("input", () => { applyTheme({ ...current, preset: "custom", accent: custom.value }); draw(); });
  custom.addEventListener("change", async () => { await saveTheme({ ...current, preset: "custom", accent: custom.value }); draw(); onSaved?.(); });
  draw();
  box.append(...dots, customDot);
  return box;
}

// Before the first paint, from what this device last used, so nothing flashes orange.
try {
  const saved = JSON.parse(localStorage.getItem(KEY) || "null");
  if (saved && /^#[0-9a-f]{6}$/i.test(saved.accent)) applyTheme(saved);
} catch (e) { /* first run, or storage blocked */ }
