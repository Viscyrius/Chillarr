// Draws subtitles the way VLC does: the file's own font, size, colours,
// outline and placement, with no box behind the text. ASS/SSA (most anime)
// is read as is; anything else comes as WebVTT. Override tags ({\i1},
// {\an8}, {\pos(x,y)} ...) are applied where they matter and never shown.

const TIME = /(\d+):(\d{2}):(\d{2})[.,](\d{1,3})/;

// Subtitle size from Settings: 1 is the file's own size (or VLC's default).
let scale = 1;
try { scale = Math.min(2, Math.max(0.6, Number(localStorage.getItem("chillarr-sub-scale")) || 1)); } catch (e) { /* storage blocked */ }
const renderers = new Set();
/** Changes the size at once on every player that is showing subtitles. */
export function setSubScale(x) {
  scale = Math.min(2, Math.max(0.6, x));
  try { localStorage.setItem("chillarr-sub-scale", String(scale)); } catch (e) { /* storage blocked */ }
  renderers.forEach((r) => r.draw(true));
}
export const subScale = () => scale;
function seconds(t) {
  const m = TIME.exec(t);
  if (!m) return 0;
  return +m[1] * 3600 + +m[2] * 60 + +m[3] + +m[4].padEnd(3, "0") / 1000;
}

/** &HAABBGGRR (ASS) to a CSS colour. */
function assColour(v, fallback) {
  const m = /&H([0-9a-f]{2})?([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})/i.exec(v || "");
  if (!m) return fallback;
  const a = 1 - parseInt(m[1] || "00", 16) / 255;
  return `rgba(${parseInt(m[4], 16)},${parseInt(m[3], 16)},${parseInt(m[2], 16)},${a.toFixed(2)})`;
}

/** Splits an ASS text into runs of { text, i, b, colour }, plus line-wide { an, pos }. */
function assRuns(raw, style) {
  const runs = [];
  const line = { an: null, pos: null, drawing: false };
  let cur = { i: style.italic, b: style.bold, colour: null };
  const re = /\{([^}]*)\}|([^{]+)/g;
  let m;
  while ((m = re.exec(raw))) {
    if (m[1] !== undefined) {
      for (const tag of m[1].split("\\").slice(1)) {
        let t;
        if ((t = /^an(\d)/.exec(tag))) line.an = +t[1];
        else if ((t = /^a(\d+)/.exec(tag))) line.an = { 1: 1, 2: 2, 3: 3, 5: 7, 6: 8, 7: 9, 9: 4, 10: 5, 11: 6 }[+t[1]] || null;
        else if ((t = /^pos\(\s*([-\d.]+)\s*,\s*([-\d.]+)/.exec(tag))) line.pos = [+t[1], +t[2]];
        else if ((t = /^move\(\s*([-\d.]+)\s*,\s*([-\d.]+)/.exec(tag))) line.pos = [+t[1], +t[2]];
        else if ((t = /^p(\d)/.exec(tag))) line.drawing = +t[1] > 0;
        else if ((t = /^i(\d)/.exec(tag))) cur = { ...cur, i: t[1] !== "0" };
        else if ((t = /^b(\d+)/.exec(tag))) cur = { ...cur, b: t[1] !== "0" };
        else if ((t = /^1?c&H([0-9a-f]+)&?/i.exec(tag))) cur = { ...cur, colour: assColour(`&H00${t[1].padStart(6, "0").slice(-6)}`, null) };
        else if (tag === "r") cur = { i: style.italic, b: style.bold, colour: null };
      }
    } else if (!line.drawing) {
      const text = m[2].replace(/\\N/g, "\n").replace(/\\n/g, " ").replace(/\\h/g, " ");
      if (text) runs.push({ ...cur, text });
    }
  }
  return { runs, line };
}

/** Reads an ASS/SSA file into cues and styles. */
export function parseAss(text) {
  const styles = {};
  const cues = [];
  let playResY = 288, playResX = 384, section = "", styleFmt = [], eventFmt = [];
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim();
    if (line.startsWith("[")) { section = line.toLowerCase(); continue; }
    if (section === "[script info]") {
      const m = /^PlayRes([XY])\s*:\s*(\d+)/i.exec(line);
      if (m) m[1].toUpperCase() === "Y" ? (playResY = +m[2]) : (playResX = +m[2]);
    } else if (section.includes("styles")) {
      if (line.startsWith("Format:")) styleFmt = line.slice(7).split(",").map((s) => s.trim().toLowerCase());
      else if (line.startsWith("Style:")) {
        const v = line.slice(6).split(",").map((s) => s.trim());
        const f = (k) => v[styleFmt.indexOf(k)];
        styles[f("name")] = {
          font: f("fontname") || "sans-serif",
          size: +f("fontsize") || 20,
          colour: assColour(f("primarycolour"), "#fff"),
          outlineColour: assColour(f("outlinecolour"), "#000"),
          outline: +(f("outline") ?? 2),
          shadow: +(f("shadow") ?? 0),
          bold: f("bold") === "-1" || f("bold") === "1",
          italic: f("italic") === "-1" || f("italic") === "1",
          an: +f("alignment") || 2,
          marginV: +f("marginv") || 10,
          marginL: +f("marginl") || 10,
          marginR: +f("marginr") || 10,
          box: f("borderstyle") === "3",
        };
      }
    } else if (section === "[events]") {
      if (line.startsWith("Format:")) eventFmt = line.slice(7).split(",").map((s) => s.trim().toLowerCase());
      else if (line.startsWith("Dialogue:")) {
        const parts = line.slice(9).split(",");
        const n = eventFmt.length || 10;
        const v = parts.slice(0, n - 1).map((s) => s.trim());
        v.push(parts.slice(n - 1).join(","));
        const f = (k) => v[eventFmt.indexOf(k)];
        const style = styles[f("style")] || styles.Default || Object.values(styles)[0] || { font: "sans-serif", size: 20, colour: "#fff", outlineColour: "#000", outline: 2, shadow: 0, an: 2, marginV: 10, marginL: 10, marginR: 10 };
        const { runs, line: l } = assRuns(f("text") || "", style);
        if (!runs.length || !runs.some((r) => r.text.trim())) continue;
        cues.push({ start: seconds(f("start")), end: seconds(f("end")), layer: +f("layer") || 0, style, runs, an: l.an || style.an, pos: l.pos, marginV: +f("marginv") || style.marginV });
      }
    }
  }
  cues.sort((a, b) => a.start - b.start);
  return { cues, playResY, playResX };
}

/** Reads WebVTT into the same shape, with plain styling. */
export function parseVtt(text) {
  const style = { font: "", size: 0, colour: "#fff", outlineColour: "#000", outline: 0, shadow: 0, an: 2, marginV: 0 };
  const cues = [];
  for (const block of text.replace(/\r/g, "").split(/\n\n+/)) {
    const lines = block.split("\n");
    const ti = lines.findIndex((l) => l.includes("-->"));
    if (ti < 0) continue;
    const [a, b] = lines[ti].split("-->");
    const body = lines.slice(ti + 1).join("\n").replace(/\{[^}]*\}/g, "");
    const runs = [];
    // <i>, <b> survive; other tags go.
    let i = false, bold = false;
    for (const part of body.split(/(<\/?[ib]>|<[^>]+>)/)) {
      if (part === "<i>") i = true; else if (part === "</i>") i = false;
      else if (part === "<b>") bold = true; else if (part === "</b>") bold = false;
      else if (!part.startsWith("<") && part) runs.push({ text: part.replace(/&amp;/g, "&").replace(/&lt;/g, "<").replace(/&gt;/g, ">").replace(/&nbsp;/g, " "), i, b: bold });
    }
    const top = /line:\s*[0-9]{1,2}%/.test(lines[ti]) && /line:\s*([0-9]{1,2})%/.exec(lines[ti])[1] < 50;
    if (runs.length) cues.push({ start: seconds(a), end: seconds(b), layer: 0, style, runs, an: top ? 8 : 2, pos: null, marginV: 0 });
  }
  return { cues, playResY: 0, playResX: 0 };
}

/**
 * Keeps an overlay over the video showing the cues due now. `box` is the
 * overlay element; it is laid over the picture itself (not the black bars).
 */
export class SubRenderer {
  constructor(video, box) {
    this.video = video;
    this.box = box;
    this.doc = null;
    this.shown = "";
    this.raf = 0;
    // Subtitle time t is shown at t * ratio + offset (timing fixes, see subsync.rs).
    this.shift = { ratio: 1, offset: 0 };
    this.tick = this.tick.bind(this);
    renderers.add(this);
    this.setVideo(video);
    window.addEventListener("resize", () => this.draw(true));
    // A phone turned while playing, or the player's box changing size by
    // itself (full screen, a window snapped): the picture moved, so do the words.
    window.addEventListener("orientationchange", () => setTimeout(() => this.draw(true), 250));
  }

  /** The <video> the words go over; a new one when the player swaps it (player.js freshVideo). */
  setVideo(video) {
    this.video = video;
    video.addEventListener("play", () => this.loop());
    video.addEventListener("seeked", () => this.draw(true));
    video.addEventListener("loadedmetadata", () => this.draw(true));
    if (window.ResizeObserver) {
      if (!this.ro) this.ro = new ResizeObserver(() => this.draw(true));
      this.ro.disconnect();
      this.ro.observe(video);
    }
  }

  set(doc) {
    this.doc = doc;
    this.draw(true);
    this.loop();
  }

  /** Moves every line: `offset` seconds later, `ratio` for another frame rate. */
  setShift(ratio = 1, offset = 0) {
    this.shift = { ratio, offset };
    this.draw(true);
  }

  clear() {
    this.doc = null;
    this.shift = { ratio: 1, offset: 0 };
    this.shown = "";
    this.box.replaceChildren();
  }

  loop() {
    if (!this.raf && this.doc) this.raf = requestAnimationFrame(this.tick);
  }

  tick() {
    this.raf = 0;
    this.draw(false);
    // Only runs while playing with subtitles on.
    if (this.doc && !this.video.paused && !this.video.ended) this.raf = requestAnimationFrame(this.tick);
  }

  /** The rectangle the picture fills inside the video element (object-fit: contain). */
  frame() {
    const v = this.video;
    const W = v.clientWidth, H = v.clientHeight;
    const vw = v.videoWidth || 16, vh = v.videoHeight || 9;
    const scale = Math.min(W / vw, H / vh);
    const w = vw * scale, h = vh * scale;
    return { left: (W - w) / 2, top: (H - h) / 2, width: w, height: h };
  }

  draw(force) {
    if (!this.doc) return;
    const t = (this.video.currentTime - this.shift.offset) / this.shift.ratio;
    const now = this.doc.cues.filter((c) => c.start <= t && c.end > t);
    const key = now.map((c) => `${c.start}|${c.end}|${c.layer}`).join(",");
    if (!force && key === this.shown) return;
    this.shown = key;
    const f = this.frame();
    Object.assign(this.box.style, { left: `${f.left}px`, top: `${f.top}px`, width: `${f.width}px`, height: `${f.height}px` });
    // While the controls are up, bottom lines rise just clear of them. When
    // the black bar under the picture is taller than the controls (a scope
    // film, a phone held upright), they don't need to move at all.
    // Measured from the seek bar, where the controls start (the bar's own box
    // is mostly a see-through fade).
    // Controller mode has its own bar, higher up: measured the same way.
    const vtop = this.video.getBoundingClientRect().top;
    const clear = (sel, prop) => {
      const e = this.box.parentElement && this.box.parentElement.querySelector(sel);
      if (e && e.offsetParent) this.box.style.setProperty(prop, `${Math.max(0, Math.round(f.top + f.height - (e.getBoundingClientRect().top - vtop) + 10))}px`);
    };
    clear(".p-seek", "--sub-lift");
    clear(".pad-bar", "--sub-lift-pad");
    const py = this.doc.playResY || 0;
    const scaleY = py ? f.height / py : 0;
    const scaleX = this.doc.playResX ? f.width / this.doc.playResX : scaleY;
    // Lines sharing a corner stack like VLC stacks them, newest closest to the edge.
    const groups = new Map();
    for (const c of now.sort((a, b) => a.layer - b.layer || a.start - b.start)) {
      const el = this.cueEl(c, scaleY, f);
      if (c.pos && scaleY) {
        const [x, y] = c.pos;
        const ax = [1, 4, 7].includes(c.an) ? 0 : [3, 6, 9].includes(c.an) ? -100 : -50;
        const ay = [7, 8, 9].includes(c.an) ? 0 : [4, 5, 6].includes(c.an) ? -50 : -100;
        Object.assign(el.style, { position: "absolute", left: `${x * scaleX}px`, top: `${y * scaleY}px`, transform: `translate(${ax}%, ${ay}%)` });
        groups.set(`pos${groups.size}`, { abs: el });
        continue;
      }
      const vert = [7, 8, 9].includes(c.an) ? "top" : [4, 5, 6].includes(c.an) ? "middle" : "bottom";
      const horiz = [1, 4, 7].includes(c.an) ? "left" : [3, 6, 9].includes(c.an) ? "right" : "center";
      const k = `${vert}-${horiz}`;
      if (!groups.has(k)) groups.set(k, { vert, horiz, margin: c.marginV * (scaleY || 0), items: [] });
      groups.get(k).items.push(el);
    }
    const out = [];
    for (const g of groups.values()) {
      if (g.abs) { out.push(g.abs); continue; }
      const stack = document.createElement("div");
      stack.className = `sub-stack ${g.vert} ${g.horiz}`;
      // In pixels, not CSS max(): Chrome before 79 (older TVs) drops max() and the line would jump to the top.
      if (g.vert === "bottom") stack.style.bottom = `${Math.max(g.margin, f.height * 0.045)}px`;
      if (g.vert === "top") stack.style.top = `${Math.max(g.margin, f.height * 0.04)}px`;
      stack.append(...g.items);
      out.push(stack);
    }
    this.box.replaceChildren(...out);
  }

  cueEl(c, scaleY, f) {
    const s = c.style;
    const el = document.createElement("div");
    el.className = "sub-line";
    // Font size: the file's size scaled to the picture, or a VLC-like default for plain subtitles.
    const size = (scaleY && s.size ? s.size * scaleY : Math.max(16, f.height * 0.055)) * scale;
    const outline = (scaleY ? Math.max(1, (s.outline || 0) * scaleY) : Math.max(1.5, size * 0.07 / scale)) * scale;
    const shadow = (scaleY ? (s.shadow || 0) * scaleY : size * 0.04 / scale) * scale;
    const oc = s.outlineColour || "#000";
    el.style.fontSize = `${size}px`;
    if (s.font) el.style.fontFamily = `"${s.font}", "Segoe UI", Arial, sans-serif`;
    el.style.color = s.colour || "#fff";
    // An outline drawn around the letters (like VLC), never a box behind them.
    el.style.webkitTextStroke = `${outline * 2}px ${oc}`;
    el.style.paintOrder = "stroke fill";
    el.style.textShadow = shadow > 0 ? `${shadow}px ${shadow}px ${Math.max(1, shadow)}px rgba(0,0,0,0.55)` : "none";
    for (const r of c.runs) {
      const span = document.createElement("span");
      span.textContent = r.text;
      if (r.i) span.style.fontStyle = "italic";
      span.style.fontWeight = r.b ? "700" : "500";
      if (r.colour) span.style.color = r.colour;
      el.append(span);
    }
    return el;
  }
}
