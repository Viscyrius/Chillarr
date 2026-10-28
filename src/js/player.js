// The in-app player. Jellyfin decides how to send each file: as-is when the
// app can play it, or converted on the fly into an HLS stream (played with hls.js).
import { $, ask, el, fmtTime, icon, invoke, listen, screenMode, store, toast, windowFull } from "./api.js";
import { SubRenderer, parseAss, parseVtt, setSubScale, subScale } from "./subrender.js";
import { buffering, caughtUp, inGroup, loadedHere, openTogether, requestPause, requestPlay, requestSeek, startedHere } from "./syncplay.js";

const box = $("#player");
// Not const: when this device turns out unable to send the sound through Web
// Audio, playback moves to a fresh element (freshVideo) without it.
let video = $("#video");
let hls = null;
let session = null; // { tile, info, playlist, watchKey }
let reportTimer = null;
let idleTimer = null;

const PLAY = "M8 5v14l11-7z";
const fill = (input) => input.style.setProperty("--fill", `${(input.value - input.min) / (input.max - input.min) * 100}%`);
const PAUSE = "M7 5h3v14H7zM14 5h3v14h-3z";
const SPEEDS = [0.5, 0.75, 1, 1.25, 1.5, 2];

// Volume as VLC has it, 0 to 300%. Past 100% the sound goes through a gain
// and a limiter (Web Audio), so loud scenes get louder without crackling.
const MAX_VOL = 3;
let level = 1;
try { level = Math.min(MAX_VOL, Math.max(0, Number(localStorage.getItem("chillarr-volume") ?? 1) || 0)); } catch (e) { /* storage blocked */ }
// Night mode: quiet talk up, explosions down (a compressor, like VLC's).
let night = false;
try { night = localStorage.getItem("chillarr-night") === "1"; } catch (e) { /* storage blocked */ }
// Sound delay for this title, in seconds; only later (see setSoundDelay).
let soundDelay = 0;
const MAX_SOUND_DELAY = 3;

// The sound only goes through Web Audio once something needs it; until then
// the video plays it as it is.
let graph = null;
/* Once an element's sound is taken into Web Audio it never comes back out,
   and where the device can't hand it over the result is silence, not just
   no night mode: an iPhone playing HLS itself (no hls.js), a TV decoding the
   sound in hardware, a context the browser keeps suspended. So the graph is
   only built where it can work, only once its context runs, and a watchdog
   (watchSilence) moves playback off it if it plays nothing but zeros. */
const AC = window.AudioContext || window.webkitAudioContext;
let nativeHls = false; // this title plays as HLS by the browser itself
const brokenWebAudio = () => { try { return localStorage.getItem("chillarr-webaudio-broken") === "1"; } catch (e) { return false; } };
/** Why night mode, sound past 100% and sound delay can't work here, or "". */
function noWebAudio() {
  if (!AC) return "Not available in this browser";
  if (brokenWebAudio() || webAudioOff) return "Not available on this device";
  if (nativeHls && !graph) return "Not available on this device";
  return "";
}
let webAudioOff = false; // this session found it silent once
let pendingCtx = null;
function armGesture(ctx) {
  // Browsers start a context only after a tap, click or key: the next one starts it.
  const go = () => { ["click", "keydown", "touchend"].forEach((t) => document.removeEventListener(t, go, true)); ctx.resume().then(route, () => {}); };
  ["click", "keydown", "touchend"].forEach((t) => document.addEventListener(t, go, true));
}
function route() {
  const boosting = level > 1;
  if (!graph && !boosting && !night && !soundDelay) return;
  if (!graph && noWebAudio()) return;
  if (!graph) {
    try {
      const ctx = pendingCtx || new AC();
      if (ctx.state !== "running") {
        // Not captured yet: the video keeps its own sound until the context runs.
        if (!pendingCtx) { pendingCtx = ctx; ctx.resume().then(route, () => {}); armGesture(ctx); }
        return;
      }
      pendingCtx = null;
      const n = { ctx, src: ctx.createMediaElementSource(video), delay: ctx.createDelay(MAX_SOUND_DELAY + 0.5), gain: ctx.createGain(),
        comp: ctx.createDynamicsCompressor(), makeup: ctx.createGain(), lim: ctx.createDynamicsCompressor(), probe: ctx.createAnalyser(), wired: "" };
      n.probe.fftSize = 2048;
      n.src.connect(n.probe); // listens only; nothing goes on from it
      n.comp.threshold.value = -32; n.comp.knee.value = 18; n.comp.ratio.value = 4; n.comp.attack.value = 0.004; n.comp.release.value = 0.35;
      n.makeup.gain.value = 2.2; // about +7 dB: what the compressor took off the loud parts, back on everything
      n.lim.threshold.value = -2; n.lim.knee.value = 0; n.lim.ratio.value = 20; n.lim.attack.value = 0.002; n.lim.release.value = 0.2;
      graph = n;
      watchSilence();
    } catch (e) { return; }
  }
  const g = graph;
  if (g.ctx.state === "suspended") g.ctx.resume().catch(() => {});
  // Only the parts in use are in the chain: at 100%, no night mode and no
  // delay, the sound is untouched.
  const chain = [g.src, g.delay, g.gain, ...(night ? [g.comp, g.makeup] : []), ...(boosting || night ? [g.lim] : []), g.ctx.destination];
  const key = `${night}|${boosting}`;
  if (g.wired !== key) {
    for (const node of [g.src, g.delay, g.gain, g.comp, g.makeup, g.lim]) node.disconnect();
    g.src.connect(g.probe);
    chain.reduce((a, b) => (a.connect(b), b));
    g.wired = key;
  }
  g.delay.delayTime.value = soundDelay;
  g.gain.gain.value = Math.max(1, level);
}

/* The watchdog: over the first seconds of real playback through the graph
   (time moving, not muted, the context running), the probe reading nothing
   but exact zeros means the device hands Web Audio silence. A film can open
   on true digital silence (a logo, a cold open on black) for a while, so it
   takes 20 seconds of playing with not one sample of sound. Playback then
   moves to a fresh element without the graph, at the same spot, and the
   device is only written off once two different titles did it. */
const SILENCE_SECONDS = 20;
let silenceTimer = 0;
function watchSilence() {
  clearInterval(silenceTimer);
  const g = graph;
  if (!g) return;
  const buf = new Float32Array(g.probe.fftSize);
  let heard = false, quietFor = 0, last = video.currentTime;
  silenceTimer = setInterval(() => {
    if (graph !== g || heard) { clearInterval(silenceTimer); return; }
    const t = video.currentTime;
    const moving = !video.paused && t > last + 0.1;
    last = t;
    if (!moving || video.muted || video.volume === 0 || g.ctx.state !== "running") return;
    g.probe.getFloatTimeDomainData(buf);
    for (let i = 0; i < buf.length; i++) if (buf[i] !== 0) { heard = true; return; }
    quietFor += 0.25;
    if (quietFor >= SILENCE_SECONDS) { clearInterval(silenceTimer); offWebAudio(); }
  }, 250);
}

function offWebAudio() {
  webAudioOff = true;
  // Only sound the browser was decoding counts against the device: a video
  // with no sound track reads zeros too, and says nothing about Web Audio.
  const decoding = !("webkitAudioDecodedByteCount" in video) || video.webkitAudioDecodedByteCount > 0;
  if (decoding && session) {
    try {
      // The titles it happened on; one silent film twice isn't the device.
      let seen = [];
      try { seen = JSON.parse(localStorage.getItem("chillarr-webaudio-silent") || "[]"); } catch (e) { seen = []; }
      if (!Array.isArray(seen)) seen = [];
      const id = session.tile.jf_id;
      if (!seen.includes(id)) seen.push(id);
      localStorage.setItem("chillarr-webaudio-silent", JSON.stringify(seen.slice(-5)));
      if (seen.length >= 2) localStorage.setItem("chillarr-webaudio-broken", "1");
    } catch (e) { /* storage blocked */ }
  }
  const s = session, at = now(), playing = !video.paused;
  freshVideo();
  // Nothing to say about a video with no sound track; the swap just keeps a TV
  // that decodes its sound in hardware (and reports none) from going silent.
  if (decoding) toast("Night mode isn't available on this device, so the sound plays as it is.");
  if (s && !box.hidden) load(s.tile, s.playlist, at, { audio: s.audio, subtitle: s.burn, watchKey: s.watchKey }).then(() => { if (!playing) video.pause(); });
}

/** A new <video> in place of the one Web Audio holds, wired up the same way. */
function freshVideo() {
  const old = video;
  const rate = old.playbackRate, defaultRate = old.defaultPlaybackRate;
  teardown();
  old.pause();
  const fresh = old.cloneNode(false);
  old.replaceWith(fresh);
  video = fresh;
  wireVideo(fresh);
  subs.setVideo(fresh);
  if (graph) { graph.ctx.close().catch(() => {}); graph = null; }
  video.volume = Math.min(1, level);
  // Properties don't come with cloneNode: the speed the user picked does.
  video.defaultPlaybackRate = defaultRate;
  video.playbackRate = rate;
}

// Controller mode state (see the bottom of this file).
const padBar = $("#pad-bar"), padPanel = $("#pad-panel"), padOsd = $("#pad-osd"), padHints = $("#pad-hints");
const pad = { on: false, i: 0, items: [], panel: null, hint: "" };
let padTimer = null, osdTimer = null, mouseAt = null;

function body(extra = {}) {
  return {
    ItemId: session.tile.jf_id,
    MediaSourceId: session.info.mediaSourceId,
    PlaySessionId: session.info.playSessionId,
    PlayMethod: session.info.playMethod,
    PositionTicks: Math.round((video.currentTime + (session.offset || 0)) * 1e7),
    IsPaused: video.paused,
    ...extra,
  };
}

function report(what) {
  if (!session) return;
  invoke("media_report", { what, body: body() }).catch(() => {});
}

function teardown() {
  clearInterval(reportTimer);
  subs.clear();
  if (hls) { hls.destroy(); hls = null; }
  video.removeAttribute("src");
  video.replaceChildren();
  video.load();
}

function nextEpisode() {
  // The show's own list (findNext) once it's in; until then the playlist.
  if (session && session.next !== undefined) return session.next;
  if (!session?.playlist) return null;
  const i = session.playlist.findIndex((e) => e.jf_id === session.tile.jf_id);
  return i >= 0 ? session.playlist[i + 1] || null : null;
}

let loadSeq = 0;

/* ---------- Subtitles ---------- */
// The language last picked (or "off"), so the next episode starts the same way.
function subPref() {
  try { return JSON.parse(localStorage.getItem("chillarr-subs") || "null"); } catch (e) { return null; }
}
function saveSubPref(p) {
  try { localStorage.setItem("chillarr-subs", JSON.stringify(p)); } catch (e) { /* storage blocked */ }
}
const isEnglish = (lang) => !lang || /^en/i.test(lang);

/** Which subtitle track to start with: the remembered language, or English when the audio isn't. */
function startingSub(info, preferExternal) {
  const subs = info.subtitles;
  if (preferExternal) {
    const ext = subs.filter((s) => s.external && s.lang === preferExternal);
    if (ext.length) return ext[ext.length - 1];
  }
  const pref = subPref();
  if (pref?.off) return null;
  const lang = pref?.lang || (isEnglish(info.audio.find((a) => a.default)?.lang || info.audio[0]?.lang) ? null : "eng");
  if (!lang) return null;
  const same = subs.filter((s) => s.lang === lang && s.url);
  return same.find((s) => !s.forced) || same[0] || null;
}

function subLabel(s) {
  const bits = [s.label || s.lang || `Track ${s.index}`];
  if (s.burn) bits.push("(drawn into the picture)");
  return bits.join(" ");
}

// Subtitles are drawn by Chillarr over the picture (see subrender.js), not by the browser.
const subs = new SubRenderer(video, $("#p-subs"));
const subCache = new Map();

/** Shows one text track (by Jellyfin index), or none. */
async function showSub(index) {
  if (session) session.sub = index;
  padRefresh();
  const s = session?.info.subtitles.find((x) => x.index === index);
  if (!s || !s.url) { subs.clear(); return; }
  const url = s.ass_url || s.url;
  let doc = subCache.get(url);
  if (!doc) {
    try {
      // Never the browser's copy: the same URL can be another file after a download.
      const text = await (await fetch(url, { cache: "no-store" })).text();
      doc = s.ass_url ? parseAss(text) : parseVtt(text);
      subCache.set(url, doc);
    } catch (e) { toast("Couldn't load those subtitles.", true); return; }
  }
  // Another track (or nothing) was picked while this one loaded.
  if (session?.sub !== index) return;
  subs.set(doc);
  const kept = shiftOf(s);
  if (kept) subs.setShift(kept.ratio, kept.offset);
  else autoSync(s, doc);
}

/* Subtitle timing. Every track is checked against the film's audio once
   (subsync.rs): a file beside the video is fixed on disk for good, a track
   inside the video is moved here. G / H (or - / + in the menu) move it by hand. */
const shiftKey = (s) => `${session?.tile.jf_id}|${s.path || `stream${s.index}`}`;
function shifts() {
  try { return JSON.parse(localStorage.getItem("chillarr-sub-shift") || "{}"); } catch (e) { return {}; }
}
function shiftOf(s) {
  return shifts()[shiftKey(s)] || null;
}
function saveShift(s, v) {
  const all = shifts();
  if (v && (v.ratio !== 1 || Math.abs(v.offset) >= 0.01)) all[shiftKey(s)] = v; else delete all[shiftKey(s)];
  const keys = Object.keys(all);
  if (keys.length > 300) delete all[keys[0]];
  try { localStorage.setItem("chillarr-sub-shift", JSON.stringify(all)); } catch (e) { /* storage blocked */ }
}
const shownTrack = () => session?.info.subtitles.find((x) => x.index === session.sub && x.url) || null;
// For the tracks menu, where `subs` is the list of tracks.
const renderer = () => subs;
const fmtShift = (o) => `${o > 0 ? "+" : o < 0 ? "-" : ""}${Math.abs(o).toFixed(1)} s`;

/** G / H and the menu's - / +: later is +. */
function nudgeSub(d) {
  const s = shownTrack();
  if (!s || !subs.doc) { osd("No subtitles on"); return; }
  const r = subs.shift.ratio, o = Math.round((subs.shift.offset + d) * 10) / 10;
  subs.setShift(r, o);
  saveShift(s, { ratio: r, offset: o });
  osd(`Subtitle delay ${fmtShift(o)}`);
  const v = $("#p-menu .p-timing-val");
  if (v) v.textContent = fmtShift(o);
}

function describeFit(f) {
  const bits = [];
  if (Math.abs(f.offset) >= 0.25) bits.push(`${Math.abs(f.offset).toFixed(1)} s ${f.offset < 0 ? "late" : "early"}`);
  if (f.ratio !== 1) bits.push("timed for another frame rate");
  return bits.join(", ");
}

/** Lines the track up with the audio. `manual`: asked from the menu, so it always says what happened. */
async function autoSync(s, doc, manual = false) {
  const ses = session;
  // Watch now plays a file that's still downloading: the audio isn't all there.
  if (!ses || ses.watchKey || !doc?.cues.length) { if (manual) toast("Syncing needs the whole video on this PC.", true); return; }
  let fit;
  try {
    fit = await invoke("subs_sync", { id: ses.tile.jf_id, index: s.index, cues: doc.cues.map((c) => [c.start, c.end]) });
  } catch (e) {
    if (manual) toast(`Couldn't sync: ${e}`, true);
    return;
  }
  if (session !== ses || ses.sub !== s.index) return;
  const off = fit.sure && (fit.ratio !== 1 || Math.abs(fit.offset) >= 0.25);
  if (fit.written) {
    // Fixed in the file itself: load it again as it is now.
    subCache.delete(s.ass_url || s.url);
    saveShift(s, null);
    toast(`Subtitles fixed to match the audio (they were ${describeFit(fit)}).`);
    showSub(s.index);
  } else if (off) {
    subs.setShift(fit.ratio, fit.offset);
    saveShift(s, { ratio: fit.ratio, offset: fit.offset });
    toast(`Subtitles lined up with the audio (they were ${describeFit(fit)}).`);
  } else if (manual) {
    if (fit.sure) { subs.setShift(1, 0); saveShift(s, null); }
    toast(fit.sure ? "These subtitles already match the audio." : "Couldn't tell from the audio. Use G and H (or - and +) to move them.");
  }
}

/** The subtitle track on screen (drawn in or overlaid), or null. */
const currentSub = () => session?.burn ?? session?.sub ?? null;

/** The audio track playing: the one picked, else the file's default. */
function currentAudio() {
  const a = session?.info.audio || [];
  return session?.audio ?? (a.find((x) => x.default) || a[0])?.index ?? null;
}

/** Subtitles off, redoing the stream when a track was drawn into it. */
function subOff() {
  saveSubPref({ off: true });
  if (session.burn != null) { report("stop"); load(session.tile, session.playlist, now(), { audio: session.audio }); } else showSub(null);
}

/** One subtitle track. Picture-based tracks need the stream redone with the track drawn in. */
function pickSub(s) {
  saveSubPref({ lang: s.lang || null });
  if (s.burn || session.burn != null) { report("stop"); load(session.tile, session.playlist, now(), { audio: session.audio, subtitle: s.burn ? s.index : null }); }
  else showSub(s.index);
}

function pickAudio(a) {
  report("stop");
  load(session.tile, session.playlist, now(), { audio: a.index, subtitle: session.burn });
}

/** Playback speed; kept for the next episode, back to normal when the player closes. */
function setSpeed(r) {
  video.defaultPlaybackRate = r;
  video.playbackRate = r;
}

function setVolume(v) {
  // Past 100% needs Web Audio; where it can't work, 100% is the top.
  const top = noWebAudio() && !graph ? 1 : MAX_VOL;
  level = Math.max(0, Math.min(top, Math.round(v * 100) / 100));
  video.muted = false;
  video.volume = Math.min(1, level);
  route();
  $("#p-volume").value = level;
  fill($("#p-volume"));
  try { localStorage.setItem("chillarr-volume", String(level)); } catch (e) { /* storage blocked */ }
  padRefresh();
  volView();
}

function setNight(on) {
  const why = noWebAudio();
  if (on && why) { osd(`Night mode: ${why.toLowerCase()}`); padRefresh(); return; }
  night = on;
  try { localStorage.setItem("chillarr-night", on ? "1" : "0"); } catch (e) { /* storage blocked */ }
  route();
  padRefresh();
  osd(on ? "Night mode on: quiet talk louder, loud parts softer" : "Night mode off");
}

/* Sound delay, per title (lips and voices out of step on some copies).
   Only "later": a browser can hold the sound back, but can't play it ahead
   of the picture. */
function soundDelays() {
  try { return JSON.parse(localStorage.getItem("chillarr-sound-delay") || "{}"); } catch (e) { return {}; }
}
const soundDelayOf = (tile) => +(soundDelays()[tile?.jf_id] || 0);
function setSoundDelay(v) {
  soundDelay = Math.max(0, Math.min(MAX_SOUND_DELAY, Math.round(v * 10) / 10));
  const all = soundDelays();
  if (soundDelay) all[session.tile.jf_id] = soundDelay; else delete all[session.tile.jf_id];
  const keys = Object.keys(all);
  if (keys.length > 300) delete all[keys[0]];
  try { localStorage.setItem("chillarr-sound-delay", JSON.stringify(all)); } catch (e) { /* storage blocked */ }
  route();
  osd(`Sound delay ${soundDelay ? `+${soundDelay.toFixed(1)} s` : "off"}`);
  const v2 = $("#p-menu .p-sound-val");
  if (v2) v2.textContent = soundDelay ? `+${soundDelay.toFixed(1)} s` : "0.0 s";
}

const volText = () => (video.muted ? "Muted" : `Volume ${Math.round(level * 100)}%`);

async function load(tile, playlist, startAt, opts = {}) {
  const { audio = null, subtitle = null, preferExternal = null, watchKey = null } = opts;
  const seq = ++loadSeq;
  $("#p-spinner").hidden = false;
  let info;
  try {
    info = await invoke("media_play", { id: tile.jf_id, start: startAt || 0, audio, subtitle });
  } catch (e) {
    if (seq !== loadSeq) return;
    toast(String(e), true);
    close(false);
    return;
  }
  // Back was pressed, or another title started, while this one was loading.
  if (seq !== loadSeq || box.hidden) return;
  teardown();
  // Jellyfin renumbers subtitle tracks when one is added, so nothing carries over.
  subCache.clear();
  // A saved volume past 100%, night mode or this title's sound delay (only ever after a click).
  soundDelay = soundDelayOf(tile);
  // The browser playing HLS itself (iPhone without hls.js): its sound can't go through Web Audio.
  nativeHls = !!info.hls && !(window.Hls && window.Hls.isSupported());
  route();
  // Jellyfin's HLS playlist covers the whole film, so both kinds of stream are
  // simply played from startAt.
  // A reload for another audio or subtitle track keeps the episode's segments.
  const same = session?.tile.jf_id === tile.jf_id ? session : null;
  session = { tile, info, playlist, offset: 0, audio, burn: subtitle, sub: null, segments: same?.segments || [], upnextDone: same?.upnextDone || false, watchKey: watchKey || same?.watchKey || null, next: same?.next };
  if (session.watchKey) {
    // The waiting screen stays up until the picture does.
    video.addEventListener("playing", () => endWait(), { once: true });
    video.addEventListener("loadeddata", () => endWait(), { once: true });
  }
  if (!same) { dropEarly(session.watchKey); hideSkip(); hideUpnext(); loadSegments(session); findNext(session); }
  $("#p-title").textContent = tile.title;
  $("#p-sub").textContent = tile.subtitle || (tile.year ? String(tile.year) : "");
  nextButton();
  subs.clear();
  if (subtitle == null) {
    const first = startingSub(info, preferExternal);
    showSub(first ? first.index : null);
  }
  padRefresh();
  if (info.hls && window.Hls?.isSupported()) {
    hls = new window.Hls({ maxBufferLength: 30, startPosition: startAt || 0 });
    hls.loadSource(info.url);
    hls.attachMedia(video);
    const mine = hls;
    hls.on(window.Hls.Events.ERROR, (_, d) => {
      if (!d.fatal) return;
      // Watch now: past what's downloaded the data just isn't there yet. Keep
      // the spinner up and ask again, never stop or close (review focus 1).
      if (session?.watchKey) {
        const media = d.type === window.Hls.ErrorTypes.MEDIA_ERROR;
        waitForData(() => { if (hls !== mine) return; if (media) mine.recoverMediaError(); else mine.startLoad(); });
        return;
      }
      toast("The stream stopped. Jellyfin may still be preparing it; try again in a moment.", true);
    });
  } else {
    video.src = info.url;
    if (startAt) video.addEventListener("loadedmetadata", () => (video.currentTime = startAt), { once: true });
  }
  if (inGroup()) {
    // In a watch party the group starts it, at the same moment for everyone.
    video.pause();
    if (same) { buffering(); video.addEventListener("canplay", caughtUp, { once: true }); }
    else if (opts.fromGroup) loadedHere();
    else startedHere(tile, startAt || 0);
  } else {
    try { await video.play(); } catch (e) { /* the user can press play */ }
  }
  report("start");
  reportTimer = setInterval(() => { report("progress"); keepWatch(); }, 10000);
}

/** Watch now: where you are, saved as it plays, so a crash or a power cut keeps it. */
function keepWatch() {
  const w = wn;
  if (!w || w.waiting || !w.uid || session?.watchKey !== w.key || video.readyState < 1) return;
  invoke("watchnow_progress", { key: w.key, position: now(), duration: total() || null, uid: w.uid }).catch(() => {});
}

/** Plays in VLC or in Chillarr, as chosen in Settings (or forced with `where`). */
export async function play(tile, playlist = null, where = null) {
  if (isWatchNow(tile)) return watchNow(tile, { where });
  tile = await resumeOrStart(tile);
  if (!tile) return;
  const useVlc = (where || store.settings?.player) === "vlc";
  if (!useVlc) return playTile(tile, playlist);
  try {
    await invoke("media_play_vlc", { id: tile.jf_id, start: tile.resume_seconds || 0, title: tile.subtitle ? `${tile.title} ${tile.subtitle}` : tile.title });
    toast("Opening in VLC. Chillarr keeps track of where you stop.");
  } catch (e) {
    toast(String(e), true);
  }
}

/**
 * Like Jellyfin: something started part way asks "Resume or Start over?",
 * unless the button pressed already said which (`chose`). Null: closed, so
 * nothing plays.
 */
async function resumeOrStart(tile) {
  const at = tile.resume_seconds || 0;
  if (tile.chose || at < 60 || (tile.duration && tile.duration - at < 60)) return tile;
  const name = tile.subtitle && !movieLike(tile) ? `${tile.title} ${tile.subtitle.split(" · ")[0]}` : tile.title;
  const yes = await ask(`Resume ${name}?`, `You stopped at ${fmtTime(at)}.`, [], `Resume from ${fmtTime(at)}`, "Start over");
  if (yes) return tile;
  return $("#ask").returnValue === "no" ? { ...tile, resume_seconds: 0 } : null;
}

export async function playTile(tile, playlist = null) {
  leaveWatch();
  box.hidden = false;
  document.body.style.overflow = "hidden";
  poke();
  await load(tile, playlist, tile.resume_seconds || 0);
}

function close(sendStop = true) {
  loadSeq++;
  if (sendStop) report("stop");
  dropEarly();
  leaveWatch();
  endWait(true);
  video.defaultPlaybackRate = 1;
  teardown();
  session = null;
  hideSkip();
  hideUpnext();
  exitPad(true);
  box.hidden = true;
  document.body.style.overflow = "";
  $("#p-menu").hidden = true;
  if (full) setFull(false);
  window.dispatchEvent(new Event("chillarr:refresh"));
}

const total = () => (session?.info.duration || video.duration || 0);
const now = () => video.currentTime + (session?.offset || 0);

function seekTo(t, fromGroup = false) {
  if (!session) return;
  const to = Math.max(0, Math.min(total() - 1, t));
  // In a watch party a skip is a request; everyone jumps when the group says.
  if (inGroup() && !fromGroup) { requestSeek(to); return; }
  // Watch now, past what's downloaded: the spinner says it's coming.
  if (wn && total() && to / total() > wn.have) $("#p-spinner").hidden = false;
  // hls.js asks Jellyfin for the right part of the film itself.
  video.currentTime = to;
}

// The big sign in the middle, shown for the play or pause a click or key asked for.
let flashWanted = 0;
function flash(d) {
  $("#p-flash-icon").setAttribute("d", d);
  const f = $("#p-flash");
  f.classList.remove("on");
  void f.offsetWidth; // restarts the animation
  f.classList.add("on");
}
const togglePlay = () => {
  flashWanted = performance.now();
  if (inGroup() && session) { if (video.paused) requestPlay(); else requestPause(); return; }
  if (video.paused) video.play().catch(() => {}); else video.pause();
};

/** What syncplay.js drives the player with. */
export const syncControl = {
  get video() { return video; },
  currentId: () => session?.tile.jf_id || null,
  currentTile: () => session?.tile || null,
  now: () => now(),
  paused: () => video.paused,
  play: () => video.play().catch(() => {}),
  pause: () => video.pause(),
  seek: (t) => seekTo(t, true),
  rate: (r) => { if (video.playbackRate !== r) video.playbackRate = r; },
  /** The party started something: open it paused, ready for the group's go. */
  open: (tile, at) => {
    leaveWatch();
    box.hidden = false;
    document.body.style.overflow = "hidden";
    poke();
    load(tile, null, at, { fromGroup: true });
  },
};
// Full screen is the window's alone. The page's own full screen on top of it
// left WebView2 deaf to the mouse, so the controls never came back.
let full = false;
async function setFull(on) {
  full = on;
  box.classList.toggle("full", on);
  await windowFull(on || screenMode.app);
  padRefresh();
}
const toggleFull = () => setFull(!full);

function playNext() {
  const n = nextEpisode();
  if (n) goTo(n);
}

/** Plays `n`, the next episode: from the playlist, or one that isn't here (`n.go`). */
function goTo(n) {
  if (n.go) { n.go(); return; }
  report("stop");
  load(n, session.playlist, 0);
}

/* ---------- Skip intro and Up next ---------- */
// Jellyfin's media segments (Intro Skipper finds them) show a Skip button
// during an intro or recap, and at the credits the next episode's card counts
// down and plays it. Without credits marked, the card comes up for the last
// 25 seconds instead.
const SKIP_LABEL = { Intro: "Skip intro", Recap: "Skip recap", Preview: "Skip preview", Commercial: "Skip ad" };
const UPNEXT_SECONDS = 10;
const FALLBACK_TAIL = 25;
const skipBtn = $("#p-skip");
const upnext = $("#p-upnext");
let upnextTimer = null;

async function loadSegments(s) {
  const segs = await invoke("media_segments", { id: s.tile.jf_id }).catch(() => []);
  if (session === s) s.segments = segs;
}

const skipTarget = (t) => session?.segments?.find((g) => SKIP_LABEL[g.kind] && t >= g.start && t < g.end - 1) || null;

/** Jumps past the intro or recap playing now. False when there is none. */
function skipNow() {
  const g = skipTarget(now());
  if (!g) return false;
  const jump = g.end - now();
  seekTo(g.end);
  hideSkip();
  osd(SKIP_LABEL[g.kind].replace("Skip", "Skipped"), `Forward ${howLong(jump)}`);
  return true;
}

function hideSkip() {
  skipBtn.classList.remove("on");
  skipBtn.dataset.seg = "";
}

/** Where the credits start: the marked ones near the end, else the last 25 seconds. */
function creditsAt() {
  const d = total();
  const outro = session?.segments?.find((g) => g.kind === "Outro" && g.end >= d - 90);
  if (outro) return outro.start;
  return d > 300 ? d - FALLBACK_TAIL : Infinity;
}

function segmentsTick(t) {
  const g = skipTarget(t);
  const key = g ? `${g.kind}@${g.start}` : "";
  if (key !== skipBtn.dataset.seg) {
    skipBtn.dataset.seg = key;
    if (g) skipBtn.textContent = SKIP_LABEL[g.kind];
    skipBtn.classList.toggle("on", !!g);
  }
  const n = nextEpisode();
  const at = creditsAt();
  if (n && !session.upnextDone && t >= at && total() - t > 1.5) showUpnext(n);
  else if (upnext.classList.contains("on") && t < at - 1) hideUpnext(); // seeked back before the credits
}

function showUpnext(n) {
  if (upnext.classList.contains("on")) return;
  const pic = n.backdrop || n.poster;
  const bar = el("i", { class: "un-fill" });
  const label = el("span", { text: `Next episode in ${UPNEXT_SECONDS}` });
  let left = UPNEXT_SECONDS;
  const playBtn = el("button", { class: "un-play", type: "button", onclick: () => { hideUpnext(); playNext(); } }, bar, label);
  upnext.replaceChildren(
    el("div", { class: "un-art", style: pic ? `background-image:url("${pic.replace(/"/g, "%22")}")` : "" }),
    el("div", { class: "un-body" },
      el("div", { class: "un-kicker", text: "Up next" }),
      el("div", { class: "un-title", text: n.subtitle || n.title }),
      n.fetch ? el("div", { class: "un-note", text: "Downloads as you watch" }) : null,
      el("div", { class: "un-actions" }, playBtn,
        el("button", { class: "un-credits", type: "button", onclick: () => hideUpnext(true) }, "Watch credits"))));
  upnext.classList.add("on");
  if (n.fetch) startEarly(n);
  requestAnimationFrame(() => (bar.style.width = `${100 / UPNEXT_SECONDS}%`));
  clearInterval(upnextTimer);
  upnextTimer = setInterval(() => {
    if (video.paused) return; // the countdown waits while paused
    left--;
    label.textContent = left > 0 ? `Next episode in ${left}` : "Starting...";
    bar.style.width = `${Math.min(100, ((UPNEXT_SECONDS - left + 1) / UPNEXT_SECONDS) * 100)}%`;
    if (left <= 0) { hideUpnext(); playNext(); }
  }, 1000);
}

/** Closes the card; `stay` keeps it closed for this episode (Watch credits). */
function hideUpnext(stay = false) {
  clearInterval(upnextTimer);
  upnext.classList.remove("on");
  if (stay && session) session.upnextDone = true;
}

skipBtn.addEventListener("click", (e) => { e.stopPropagation(); skipNow(); });

// Controls
$("#p-back").addEventListener("click", () => close());
$("#p-play").addEventListener("click", togglePlay);
$("#p-back10").addEventListener("click", () => seekBy(-10));
$("#p-fwd10").addEventListener("click", () => seekBy(10));
$("#p-volume").addEventListener("input", (e) => { setVolume(+e.target.value); volOsd(); });
$("#p-volume").value = level;
video.volume = Math.min(1, level);
fill($("#p-volume"));
$("#p-full").addEventListener("click", toggleFull);
$("#p-next").addEventListener("click", playNext);
// Like Jellyfin: a click anywhere on the picture (not on a control) plays or
// pauses, a double click goes full screen; the wheel is the volume, like VLC.
// The bars count as the picture only while they're hidden: with them up, the
// space between the buttons is part of the controls, like in Jellyfin.
const onPicture = (e) => !e.target.closest("button, input, select, label, a, .p-menu, .p-upnext, .p-wait, .p-seek, .pad-panel, .pad-bar, .p-title")
  && !(shown() && e.target.closest(".p-top, .p-bottom"));
// A finger is different: a tap on the picture shows or hides the bars (the
// big play button pauses), and a tap on the controls keeps them up.
let touched = false;
box.addEventListener("pointerdown", (e) => {
  touched = e.pointerType === "touch" || e.pointerType === "pen";
  if (touched) { overControls = false; if (!onPicture(e)) poke(); }
});
box.addEventListener("click", (e) => {
  if (pad.on || !session || !onPicture(e)) return;
  // A click away from an open menu only closes it.
  if (!$("#p-menu").hidden) return;
  // detail 0: Enter or Space on a focused control, not a finger.
  if (touched && e.detail !== 0) {
    if (box.classList.contains("idle") || video.paused) poke();
    else { clearTimeout(idleTimer); box.classList.add("idle"); }
    return;
  }
  togglePlay();
});
box.addEventListener("dblclick", (e) => { if (!pad.on && !touched && onPicture(e)) toggleFull(); });
box.addEventListener("wheel", (e) => {
  if (pad.on || !session || e.target.closest(".p-menu, .p-upnext")) return;
  e.preventDefault();
  setVolume(level + (e.deltaY < 0 ? 0.05 : -0.05));
  volOsd();
  poke();
}, { passive: false });

let dragging = false;
const seek = $("#p-seek");
seek.addEventListener("input", () => { dragging = true; fill(seek); $("#p-now").textContent = fmtTime((seek.value / 1000) * total()); });
seek.addEventListener("change", () => { dragging = false; seekTo((seek.value / 1000) * total()); });
const seekTip = $("#p-seek-tip");
seek.addEventListener("mousemove", (e) => {
  const d = total();
  if (!d) return;
  const r = seek.getBoundingClientRect();
  const x = Math.max(0, Math.min(r.width, e.clientX - r.left));
  const at = (x / r.width) * d;
  $("#p-seek-time").textContent = fmtTime(at);
  const half = tipPicture(at) ? 100 : 30;
  seekTip.style.left = `${Math.max(half, Math.min(r.width - half, x))}px`;
  seekTip.hidden = false;
});
seek.addEventListener("mouseleave", () => (seekTip.hidden = true));

/** Jellyfin's preview picture for time `at` (trickplay.rs), or false when it has none. */
function tipPicture(at) {
  const t = session?.info.trickplay;
  const pic = $("#p-seek-pic");
  if (!t || !t.count) { pic.hidden = true; return false; }
  const shown = 192, scale = shown / t.w;
  const i = Math.min(t.count - 1, Math.floor((at * 1000) / t.interval));
  const per = t.cols * t.rows, n = i % per;
  Object.assign(pic.style, {
    width: `${shown}px`, height: `${t.h * scale}px`,
    backgroundImage: `url("${t.url.replace("{n}", Math.floor(i / per))}")`,
    backgroundSize: `${t.cols * t.w * scale}px ${t.rows * t.h * scale}px`,
    backgroundPosition: `${-(n % t.cols) * t.w * scale}px ${-Math.floor(n / t.cols) * t.h * scale}px`,
  });
  pic.hidden = false;
  return true;
}

const flashFor = () => { const asked = performance.now() - flashWanted < 1500; flashWanted = 0; return asked; };
$("#p-together").addEventListener("click", (e) => { e.stopPropagation(); openTogether(); });
/** Everything the player listens to on its <video>; run again for a fresh one (freshVideo). */
function wireVideo(v) {
  v.addEventListener("timeupdate", () => {
    if (!session) return;
    const t = now(), d = total();
    if (!dragging && d) { seek.value = Math.round((t / d) * 1000); fill(seek); }
    $("#p-now").textContent = fmtTime(t);
    $("#p-left").textContent = d ? `-${fmtTime(d - t)}` : "";
    segmentsTick(t);
  });
  v.addEventListener("play", () => {
    if (flashFor()) flash(PLAY);
    if (graph?.ctx.state === "suspended") graph.ctx.resume().catch(() => {});
    $("#p-play-icon").setAttribute("d", PAUSE);
    padRefresh();
    // Playing again with the controller overlay up: let it fade on its own.
    if (pad.on && !box.classList.contains("idle")) padWake();
  });
  v.addEventListener("pause", () => { if (flashFor()) { flash(PAUSE); poke(); } $("#p-play-icon").setAttribute("d", PLAY); report("progress"); padRefresh(); });
  v.addEventListener("volumechange", () => { padRefresh(); volView(); });
  v.addEventListener("ratechange", padRefresh);
  v.addEventListener("waiting", () => { $("#p-spinner").hidden = false; if (inGroup()) buffering(); });
  v.addEventListener("playing", () => { $("#p-spinner").hidden = true; if (wn) wn.retry = null; if (inGroup()) caughtUp(); });
  // Watch now played as-is: a read past the downloaded part can fail. Wait, then
  // pick up at the same spot, with the spinner up all the while.
  v.addEventListener("error", () => {
    const s = session;
    if (!s?.watchKey || !wn || box.hidden || !video.currentSrc || hls) return;
    // A reload that failed before its first frame reads 0: keep the last real spot.
    const at = now() > 0 ? now() : wn.at;
    wn.at = at;
    waitForData(() => { if (session === s) load(s.tile, null, at, { audio: s.audio, subtitle: s.burn }); });
  });
  v.addEventListener("ended", () => {
    const n = nextEpisode();
    if (n) { goTo(n); return; }
    report("stop");
    close(false);
  });
}
wireVideo(video);

// Audio, subtitles and speed
$("#p-tracks").addEventListener("click", (e) => {
  e.stopPropagation();
  const menu = $("#p-menu");
  if (!menu.hidden) { menu.hidden = true; return; }
  const items = [el("h5", { text: "Subtitles" })];
  const subs = session?.info.subtitles || [];
  const current = currentSub();
  const off = el("button", { text: "Off", class: current == null ? "on" : "" });
  off.addEventListener("click", () => { menu.hidden = true; subOff(); });
  items.push(off);
  subs.forEach((s) => {
    const b = el("button", { text: subLabel(s), class: current === s.index ? "on" : "" });
    b.addEventListener("click", () => { menu.hidden = true; pickSub(s); });
    items.push(b);
  });
  const more = el("button", { class: "p-more", text: "Download subtitles..." });
  more.addEventListener("click", (ev) => { ev.stopPropagation(); subsPanel(menu); });
  items.push(more);
  // Subtitle size: smaller / bigger, applied at once; the menu stays open.
  const pct = el("span", { class: "p-size-val", text: `${Math.round(subScale() * 100)}%` });
  const step = (d) => (ev) => { ev.stopPropagation(); setSubScale(Math.round((subScale() + d) * 10) / 10); pct.textContent = `${Math.round(subScale() * 100)}%`; };
  items.push(el("div", { class: "p-size" }, el("span", { text: "Size" }),
    el("button", { class: "p-size-btn", "aria-label": "Smaller subtitles", onclick: step(-0.1) }, "A-"), pct,
    el("button", { class: "p-size-btn", "aria-label": "Bigger subtitles", onclick: step(0.1) }, "A+")));
  if (shownTrack()) {
    const nudge = (d) => (ev) => { ev.stopPropagation(); nudgeSub(d); };
    items.push(el("div", { class: "p-size" }, el("span", { text: "Timing" }),
      el("button", { class: "p-size-btn", "aria-label": "Subtitles earlier", title: "Earlier (G)", onclick: nudge(-0.5) }, "-"),
      el("span", { class: "p-size-val p-timing-val", text: fmtShift(renderer().shift.offset) }),
      el("button", { class: "p-size-btn", "aria-label": "Subtitles later", title: "Later (H)", onclick: nudge(0.5) }, "+")));
    const sync = el("button", { class: "p-more p-sync", text: "Sync to the audio" });
    sync.addEventListener("click", async (ev) => {
      ev.stopPropagation();
      const s = shownTrack();
      if (!s || !renderer().doc) return;
      sync.disabled = true;
      sync.textContent = "Listening to the audio...";
      saveShift(s, null);
      await autoSync(s, renderer().doc, true);
      sync.disabled = false;
      sync.textContent = "Sync to the audio";
      const v = $("#p-menu .p-timing-val");
      if (v) v.textContent = fmtShift(renderer().shift.offset);
    });
    items.push(sync);
  }
  items.push(el("h5", { text: "Audio" }));
  if (session?.info.audio.length > 1) {
    session.info.audio.forEach((a) => {
      const b = el("button", { text: a.label || `Track ${a.index}`, class: session.audio === a.index ? "on" : "" });
      b.addEventListener("click", () => { menu.hidden = true; pickAudio(a); });
      items.push(b);
    });
  }
  const nightWhy = noWebAudio();
  const nightBtn = el("button", { class: `p-toggle${night && !nightWhy ? " on" : ""}`, title: "Night mode (N)", disabled: !!nightWhy }, el("span", { class: "p-toggle-text" }, el("span", { text: "Night mode" }), el("small", { text: nightWhy || "Quiet talk louder, loud parts softer" })));
  nightBtn.addEventListener("click", (ev) => { ev.stopPropagation(); setNight(!night); nightBtn.classList.toggle("on", night); });
  items.push(nightBtn);
  const later = (d) => (ev) => { ev.stopPropagation(); setSoundDelay(soundDelay + d); };
  items.push(el("div", { class: "p-size", title: "When voices come before the lips move" }, el("span", { text: "Sound delay" }),
    el("button", { class: "p-size-btn", "aria-label": "Sound sooner", onclick: later(-0.1) }, "-"),
    el("span", { class: "p-size-val p-sound-val", text: soundDelay ? `+${soundDelay.toFixed(1)} s` : "0.0 s" }),
    el("button", { class: "p-size-btn", "aria-label": "Sound later", onclick: later(0.1) }, "+")));
  items.push(el("h5", { text: "Speed" }));
  items.push(el("div", { class: "p-speeds" }, SPEEDS.map((r) => el("button", {
    text: `${r}x`, class: video.playbackRate === r ? "on" : "",
    onclick: () => { menu.hidden = true; setSpeed(r); },
  }))));
  menu.replaceChildren(...items);
  menu.hidden = false;
});
document.addEventListener("click", (e) => { if (!e.target.closest(".p-menu-wrap")) $("#p-menu").hidden = true; });

const LANGS = [["eng", "English"], ["spa", "Spanish"], ["por", "Portuguese"], ["fre", "French"], ["ger", "German"], ["ita", "Italian"], ["ara", "Arabic"], ["tur", "Turkish"], ["rus", "Russian"], ["pol", "Polish"], ["dut", "Dutch"], ["jpn", "Japanese"], ["kor", "Korean"], ["chi", "Chinese"], ["hin", "Hindi"], ["ind", "Indonesian"]];
const SETUP_NOTE = "Downloading subtitles needs a free OpenSubtitles.com account. Add it once in Settings and Chillarr does the rest.";

/** One line about a search result: format, downloads, and its quirks. */
const resultMeta = (f) => [f.format?.toUpperCase(), f.downloads != null ? `${f.downloads.toLocaleString()} downloads` : null, f.hearing_impaired ? "for the hard of hearing" : null, f.machine ? "machine translated" : null].filter(Boolean).join(" · ");

/** Downloads one result into Jellyfin and restarts the stream with it. False when the title changed meanwhile. */
async function addSub(s, f, lang) {
  await invoke("subs_download", { id: s.tile.jf_id, sub: f.id });
  if (session !== s) return false;
  saveSubPref({ lang });
  toast("Subtitles added.");
  report("stop");
  load(s.tile, s.playlist, now(), { audio: s.audio, preferExternal: lang });
  return true;
}

/** Inside the tracks menu: search OpenSubtitles for this file and add one. */
function subsPanel(menu) {
  const s = session;
  if (!s) return;
  const pref = subPref();
  const lang = el("select", { class: "p-lang", "aria-label": "Language" }, LANGS.map(([v, n]) => el("option", { value: v, text: n, selected: v === (pref?.lang || "eng") })));
  const list = el("div", { class: "p-results" });
  const back = el("button", { text: "Back" });
  back.addEventListener("click", (ev) => { ev.stopPropagation(); menu.hidden = true; $("#p-tracks").click(); });
  async function find() {
    list.replaceChildren(el("p", { class: "p-note", text: "Searching OpenSubtitles..." }));
    let found;
    try { found = await invoke("subs_search", { id: s.tile.jf_id, lang: lang.value }); } catch (e) {
      if (session !== s) return;
      if (String(e) === "setup") {
        list.replaceChildren(el("p", { class: "p-note", text: SETUP_NOTE }),
          el("button", { class: "p-more", text: "Open Settings", onclick: () => { close(); location.hash = "#settings/account"; } }));
      } else list.replaceChildren(el("p", { class: "p-note", text: String(e) }));
      return;
    }
    if (session !== s) return;
    if (!found.length) { list.replaceChildren(el("p", { class: "p-note", text: "Nothing found in this language." })); return; }
    list.replaceChildren(...found.slice(0, 30).map((f) => {
      const meta = resultMeta(f);
      const b = el("button", { class: "p-result" }, el("span", { class: "p-rname", text: f.name }), el("small", { text: meta }));
      b.addEventListener("click", async (ev) => {
        ev.stopPropagation();
        b.disabled = true;
        b.querySelector("small").textContent = "Downloading...";
        try {
          if (await addSub(s, f, lang.value)) menu.hidden = true;
        } catch (err) { toast(String(err), true); b.disabled = false; b.querySelector("small").textContent = meta; }
      });
      return b;
    }));
  }
  lang.addEventListener("change", find);
  lang.addEventListener("click", (ev) => ev.stopPropagation());
  menu.replaceChildren(back, el("h5", { text: "Download subtitles" }), lang, list);
  find();
}

// Hide the controls when the mouse rests, like YouTube: any move brings them
// back, and they stay up while the mouse is on them (or a menu is open).
let overControls = false;
function poke() {
  box.classList.remove("idle");
  clearTimeout(idleTimer);
  idleTimer = setTimeout(() => {
    if (overControls || !$("#p-menu").hidden) { poke(); return; }
    if (!video.paused) box.classList.add("idle");
  }, 2800);
}
document.addEventListener("mouseleave", () => (overControls = false));
window.addEventListener("mousemove", (e) => {
  if (box.hidden) return;
  overControls = !!e.target.closest?.(".p-bottom, .p-top, .p-menu-wrap, .p-upnext, .p-skip");
  if (pad.on) {
    // Only a real move hands the player back to the mouse; the first event
    // after switching may just be the browser re-checking the hover.
    const at = [e.screenX, e.screenY];
    if (!mouseAt) { mouseAt = at; return; }
    if (Math.abs(at[0] - mouseAt[0]) + Math.abs(at[1] - mouseAt[1]) < 8) return;
    exitPad();
  }
  poke();
});

window.addEventListener("keydown", (e) => {
  if (box.hidden) return;
  if (e.isTrusted) exitPad();
  // The menu's language list, or a shortcut with Ctrl / Alt, isn't ours.
  if (e.target.tagName === "SELECT" || e.ctrlKey || e.altKey || e.metaKey) return;
  // A TV remote's media and Back keys (LG webOS codes, Tizen's Back, the standard names).
  const tv = { 415: "play", 19: "pause", 413: "stop", 412: "rewind", 417: "forward", 461: "Escape", 10009: "Escape", 10252: "playpause" }[e.keyCode]
    || { MediaPlayPause: "playpause", MediaPlay: "play", MediaPause: "pause", MediaStop: "stop", MediaRewind: "rewind", MediaFastForward: "forward", GoBack: "Escape", BrowserBack: "Escape" }[e.key];
  // A remote's other buttons can come with no key name at all.
  const k = tv || (!e.key ? "" : e.key.length === 1 ? e.key.toLowerCase() : e.key);
  let used = true;
  if (k === "play") { if (video.paused) togglePlay(); }
  else if (k === "pause") { if (!video.paused) togglePlay(); }
  else if (k === "playpause") togglePlay();
  else if (k === "stop") close();
  else if (k === "rewind") seekBy(-10);
  else if (k === "forward") seekBy(10);
  else if (k === " " || k === "k") togglePlay();
  else if (k === "ArrowLeft" || k === "j") seekBy(-10);
  else if (k === "ArrowRight" || k === "l") seekBy(10);
  else if (k === "ArrowUp") { setVolume(level + 0.05); volOsd(); }
  else if (k === "ArrowDown") { setVolume(level - 0.05); volOsd(); }
  else if (k === "m") { video.muted = !video.muted; volOsd(); }
  else if (k === "n") setNight(!night);
  else if (k === "f" || k === "F11") toggleFull();
  else if (k === "s") skipNow();
  else if (k === "g") nudgeSub(-0.1);
  else if (k === "h") nudgeSub(0.1);
  else if (/^[0-9]$/.test(k) && total()) { seekTo(total() * (+k / 10)); osd(`Jumped to ${+k * 10}%`, `${fmtTime(total() * (+k / 10))} / ${fmtTime(total())}`); }
  // Esc steps back one thing at a time: the menu, then full screen, then the player.
  else if (k === "Escape") { if (!$("#p-menu").hidden) $("#p-menu").hidden = true; else if (full) setFull(false); else close(); }
  else used = false;
  // Arrows and Space would also move a focused slider or press a focused button.
  if (used) e.preventDefault();
  // Phone remote keys arrive here too; with a controller in hand, show its overlay.
  if (pad.on) padWake(); else poke();
});

/* ---------- Controller mode ---------- */
// Picking up a game pad swaps the mouse controls for a bar of big buttons,
// list panels for tracks and speed, and shortcuts that work with it hidden.
// nav.js hands every pad button to padInput() while the player is on top.
const PAD_HIDE = 4000;
const PATHS = {
  back10: ["M11 6 5 12l6 6M19 6l-6 6 6 6"],
  fwd10: ["m13 6 6 6-6 6M5 6l6 6-6 6"],
  subs: ["M3 7a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z", "M7 12h4M13 12h4M7 15h7"],
  audio: ["M9 18V5l11-2v13", "M9 18a3 3 0 1 1-6 0 3 3 0 0 1 6 0zM20 16a3 3 0 1 1-6 0 3 3 0 0 1 6 0z"],
  speed: ["M4.5 17a8 8 0 1 1 15 0", "M12 14l4.5-4.5"],
  volume: ["M4 9v6h4l5 4V5L8 9z", "M16 9a4 4 0 0 1 0 6M18.5 6.5a8 8 0 0 1 0 11"],
  next: ["M6 5v14l10-7z", "M18 5v14"],
  full: ["M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5"],
  close: ["M6 6l12 12M18 6 6 18"],
};
const shown = () => !box.classList.contains("idle");
const trackName = (t) => t.label || t.lang || `Track ${t.index}`;
const speedName = (r) => (r === 1 ? "Normal" : `${r}x`);

/** What the bar holds right now, left to right. */
function barItems() {
  const s = session;
  const sub = s?.info.subtitles.find((t) => t.index === currentSub());
  const aud = s?.info.audio.find((a) => a.index === currentAudio());
  const items = [
    { key: "play", label: video.paused ? "Play" : "Pause", paths: [video.paused ? PLAY : PAUSE], solid: true, run: togglePlay },
    { key: "back10", label: "Back 10s", paths: PATHS.back10, run: () => seekBy(-10) },
    { key: "fwd10", label: "Forward 10s", paths: PATHS.fwd10, run: () => seekBy(10) },
    { key: "subs", label: "Subtitles", value: sub ? trackName(sub) : "Off", paths: PATHS.subs, run: subsList },
    { key: "audio", label: "Audio", value: aud ? trackName(aud) : "Default", paths: PATHS.audio, run: audioList },
    { key: "speed", label: "Speed", value: speedName(video.playbackRate), paths: PATHS.speed, run: speedList },
    { key: "subsize", label: "Text size", value: `${Math.round(subScale() * 100)}%`, paths: PATHS.subs, run: sizeList },
    { key: "night", label: "Night mode", value: noWebAudio() ? "Not available" : night ? "On" : "Off", paths: PATHS.volume, run: () => setNight(!night) },
    { key: "volume", label: "Volume", value: `${Math.round(level * 100)}%`, paths: PATHS.volume, run: volumePanel },
  ];
  const n = nextEpisode();
  if (n) items.push({ key: "next", label: "Next episode", value: [epTag(n), n.fetch ? "Downloads" : null].filter(Boolean).join(" · ") || null, paths: PATHS.next, run: playNext });
  items.push({ key: "full", label: full ? "Exit full screen" : "Full screen", paths: PATHS.full, run: toggleFull });
  items.push({ key: "close", label: "Close", paths: PATHS.close, run: () => close() });
  return items;
}

/** Redraws the bar, keeping the same button selected. Only on changes, never per frame. */
function renderBar() {
  const key = pad.items[pad.i]?.key;
  pad.items = barItems();
  const j = pad.items.findIndex((x) => x.key === key);
  pad.i = j >= 0 ? j : Math.min(pad.i, pad.items.length - 1);
  padBar.replaceChildren(...pad.items.map((it, n) => el("button", {
    class: `pad-btn${it.solid ? " solid" : ""}${n === pad.i ? " sel" : ""}`, tabindex: "-1",
    onclick: () => { pad.i = n; it.run(); },
  }, icon(...it.paths), el("span", { class: "pad-label", text: it.label }), it.value ? el("small", { text: it.value }) : null)));
}

function padRefresh() {
  if (pad.on) renderBar();
}

function barSel(n) {
  pad.i = Math.max(0, Math.min(pad.items.length - 1, n));
  [...padBar.children].forEach((b, k) => b.classList.toggle("sel", k === pad.i));
}

/** The button glyphs along the bottom, for what the pad does right now. */
function hints(kind) {
  if (pad.hint === kind) return;
  pad.hint = kind;
  const sets = {
    bar: [[["A"], "Select"], [["B"], "Hide"], [["X"], "Play"], [["Y"], "Subtitles"], [["LT", "RT"], "Seek"], [["LB", "RB"], "Subtitle track"]],
    list: [[["A"], "Pick"], [["B"], "Back"], [["X"], "Play"], [["LT", "RT"], "Seek"]],
    volume: [[["D-pad"], "Volume"], [["A"], "Done"], [["B"], "Back"]],
  };
  padHints.replaceChildren(...sets[kind].map(([keys, label]) => el("span", { class: "pad-hint" },
    keys.map((k) => el("kbd", { class: `${k.length === 1 ? "round " : ""}g-${k.toLowerCase()}`, text: k })), el("span", { text: label }))));
}

/** Shows the overlay and restarts its fade-out (none while a panel is open). */
function padWake() {
  box.classList.remove("idle");
  clearTimeout(padTimer);
  if (!pad.panel) padTimer = setTimeout(() => { if (pad.on && !pad.panel && !video.paused) box.classList.add("idle"); }, PAD_HIDE);
}

function padHide() {
  clearTimeout(padTimer);
  box.classList.add("idle");
}

/** A short note at the top of the picture (skips, volume, subtitle delay), like VLC's.
 *  With a controller, only for shortcuts used while its overlay is hidden. */
const OSD_FOR = 3000;
function osd(text, small = null, meter = null) {
  if (pad.on && shown()) return;
  const kids = [el("strong", { text })];
  if (small) kids.push(el("small", { text: small }));
  if (meter != null) {
    // VLC-style level bar; the mark is 100%.
    kids.push(el("span", { class: "osd-meter" }, el("i", { style: `width:${Math.max(0, Math.min(1, meter)) * 100}%` }), el("b", { style: `left:${100 / MAX_VOL}%` })));
  }
  padOsd.replaceChildren(...kids);
  padOsd.classList.add("on");
  clearTimeout(osdTimer);
  osdTimer = setTimeout(() => padOsd.classList.remove("on"), OSD_FOR);
}

/** The volume note: "Volume 125%" over a level bar. */
const volOsd = () => osd(volText(), null, video.muted ? 0 : level / MAX_VOL);

function enterPad() {
  pad.on = true;
  mouseAt = null;
  box.classList.add("pad-mode");
  clearTimeout(idleTimer);
  $("#p-menu").hidden = true;
  if (box.contains(document.activeElement)) document.activeElement.blur();
  pad.i = 0;
  pad.hint = "";
  renderBar();
  hints("bar");
  subs.draw(true); // the controller bar sits higher: subtitles measure it
  // Start hidden: the first A brings the overlay up instead of pausing.
  padHide();
}

function exitPad(quiet = false) {
  if (!pad.on) return;
  pad.on = false;
  pad.panel = null;
  padPanel.hidden = true;
  clearTimeout(padTimer);
  padOsd.classList.remove("on");
  box.classList.remove("pad-mode");
  if (!quiet) poke();
}

// Presses in a row add up while the note is up, so it says how far in all:
// "Forward 30 seconds".
const howLong = (sec) => (sec < 60 ? `${Math.round(sec)} second${Math.round(sec) === 1 ? "" : "s"}` : fmtTime(sec));
const seekRun = { sum: 0, at: 0, to: 0 };
function seekBy(d) {
  if (!session) return;
  const t = performance.now();
  const going = t - seekRun.at < OSD_FOR && Math.sign(seekRun.sum) === Math.sign(d);
  seekRun.sum = going ? seekRun.sum + d : d;
  // In a watch party the jump lands later, so count from where it's headed.
  const from = going ? seekRun.to : now();
  seekRun.to = Math.max(0, Math.min(total() - 1, from + d));
  seekRun.at = t;
  seekTo(seekRun.to);
  osd(`${seekRun.sum > 0 ? "Forward" : "Back"} ${howLong(Math.abs(seekRun.sum))}`, `${fmtTime(seekRun.to)} / ${fmtTime(total())}`);
}

function volumeBy(d) {
  setVolume(level + d);
  volOsd();
}

/** LB/RB: the previous or next text subtitle track, with Off in the loop. */
function cycleSub(step) {
  const s = session;
  if (!s) return;
  // Picture-based tracks are left out: each one restarts the stream.
  const ring = [null, ...s.info.subtitles.filter((t) => !t.burn).map((t) => t.index)];
  if (ring.length < 2) { osd("No other subtitles"); return; }
  const at = ring.indexOf(currentSub());
  const next = ring[((at < 0 ? 0 : at) + step + ring.length) % ring.length];
  const t = s.info.subtitles.find((x) => x.index === next);
  if (t) pickSub(t); else subOff();
  osd(t ? `Subtitles: ${trackName(t)}` : "Subtitles off");
}

/* Panels: a list over the bar, or the volume meter. */

/** Opens a list panel. Items are { label, meta, on, accent, run }; B runs `back`. */
function openList(title, items, back = closePanel, note = null, start = null) {
  const i = start ?? Math.max(0, items.findIndex((x) => x.on));
  pad.panel = { kind: "list", title, items, i, back, note };
  renderPanel();
}

function renderPanel() {
  const p = pad.panel;
  const kids = [el("h5", { text: p.title })];
  if (p.note) kids.push(el("p", { class: "pad-note", text: p.note }));
  if (p.kind === "volume") {
    kids.push(el("div", { class: "pad-vol" }, icon(...PATHS.volume), el("div", { class: "pad-meter" }, el("span")), el("strong")));
  } else if (p.items.length) {
    kids.push(el("div", { class: "pad-list" }, p.items.map((it, n) => {
      it.el = el("button", {
        class: `pad-item${it.on ? " on" : ""}${it.accent ? " accent" : ""}`, tabindex: "-1",
        onclick: () => { p.i = n; pick(); },
      }, el("span", { text: it.label }), it.meta ? el("small", { text: it.meta }) : null);
      return it.el;
    })));
  }
  padPanel.replaceChildren(...kids);
  clearTimeout(padTimer);
  box.classList.remove("idle");
  // Sit above the bar, centred on the button that opened it.
  const bar = padBar.getBoundingClientRect();
  const b = padBar.children[pad.i]?.getBoundingClientRect() || bar;
  const w = Math.min(400, innerWidth - 32);
  padPanel.style.width = `${w}px`;
  padPanel.style.left = `${Math.max(16, Math.min(innerWidth - w - 16, b.left + b.width / 2 - w / 2))}px`;
  padPanel.style.bottom = `${innerHeight - bar.top + 16}px`;
  padPanel.style.maxHeight = `${Math.max(160, bar.top - 100)}px`;
  padPanel.hidden = false;
  hints(p.kind === "volume" ? "volume" : "list");
  if (p.kind === "volume") volView(); else listSel(p.i);
}

function closePanel() {
  pad.panel = null;
  padPanel.hidden = true;
  hints("bar");
  padWake();
}

function listSel(n) {
  const p = pad.panel;
  if (!p.items.length) return;
  p.i = Math.max(0, Math.min(p.items.length - 1, n));
  p.items.forEach((it, k) => it.el?.classList.toggle("sel", k === p.i));
  // Keep the choice in view inside the list (the panel itself never scrolls the page).
  const b = p.items[p.i].el, list = b?.parentElement;
  if (!list) return;
  if (b.offsetTop < list.scrollTop) list.scrollTo({ top: b.offsetTop - 6, behavior: "smooth" });
  else if (b.offsetTop + b.offsetHeight > list.scrollTop + list.clientHeight) list.scrollTo({ top: b.offsetTop + b.offsetHeight - list.clientHeight + 6, behavior: "smooth" });
}

function pick() {
  const it = pad.panel?.items[pad.panel.i];
  if (it && !it.busy) it.run(it);
}

/** Selects a bar button by key, so a panel opened by a shortcut sits over it. */
function barTo(key) {
  const n = pad.items.findIndex((x) => x.key === key);
  if (n >= 0) barSel(n);
}

function subsList(atDownload = false) {
  const s = session;
  if (!s) return;
  barTo("subs");
  const cur = currentSub();
  const items = [{ label: "Off", on: cur == null, run: (it) => { if (currentSub() != null) subOff(); markOn(it); } }];
  s.info.subtitles.forEach((t) => items.push({
    label: trackName(t),
    meta: [t.lang && t.label ? t.lang.toUpperCase() : null, t.forced ? "Forced" : null, t.external ? "Added" : null, t.burn ? "Drawn into the picture" : null].filter(Boolean).join(" · "),
    on: cur === t.index,
    run: (it) => { if (currentSub() !== t.index) pickSub(t); markOn(it); },
  }));
  items.push({ label: "Download subtitles...", accent: true, run: () => langList() });
  openList("Subtitles", items, closePanel, null, atDownload === true ? items.length - 1 : null);
}

function audioList() {
  const s = session;
  if (!s) return;
  barTo("audio");
  const cur = currentAudio();
  const items = s.info.audio.map((a) => ({
    label: trackName(a),
    meta: a.lang && a.label ? a.lang.toUpperCase() : null,
    on: cur === a.index,
    run: (it) => { if (currentAudio() !== a.index) pickAudio(a); markOn(it); },
  }));
  openList("Audio", items, closePanel, items.length ? (items.length < 2 ? "This file has one audio track." : null) : "No audio tracks listed.");
}

function speedList() {
  barTo("speed");
  openList("Speed", SPEEDS.map((r) => ({ label: speedName(r), on: video.playbackRate === r, run: (it) => { setSpeed(r); markOn(it); } })));
}

const SUB_SIZES = [0.6, 0.8, 1, 1.2, 1.4, 1.6, 1.8, 2];
function sizeList() {
  barTo("subsize");
  const now = Math.round(subScale() * 10) / 10;
  openList("Subtitle size", SUB_SIZES.map((x) => ({ label: `${Math.round(x * 100)}%`, on: now === x, run: (it) => { setSubScale(x); markOn(it); } })));
}

/** Moves the "current" dot in a list to the item just picked; the panel stays open. */
function markOn(it) {
  for (const x of pad.panel?.items || []) { x.on = x === it; x.el?.classList.toggle("on", x.on); }
}

function volumePanel() {
  barTo("volume");
  pad.panel = { kind: "volume", title: "Volume", items: [], i: 0, back: closePanel };
  renderPanel();
}

/** Keeps the volume panel's meter and number current. */
function volView() {
  if (pad.panel?.kind !== "volume") return;
  const pct = Math.round(level * 100);
  const m = padPanel.querySelector(".pad-meter span");
  if (m) m.style.transform = `scaleX(${level / MAX_VOL})`;
  const n = padPanel.querySelector(".pad-vol strong");
  if (n) n.textContent = `${pct}%`;
}

/** Download subtitles, step one: the language. */
function langList(lang = null) {
  const s = session;
  if (!s) return;
  const want = lang || subPref()?.lang || "eng";
  openList("Download subtitles", LANGS.map(([v, n]) => ({ label: n, on: v === want, run: () => resultsList(s, v, n) })), () => subsList(true), "Pick a language.");
}

/** Download subtitles, step two: what OpenSubtitles has in that language. */
async function resultsList(s, lang, name) {
  const title = `${name} subtitles`;
  const back = () => langList(lang);
  openList(title, [], back, "Searching OpenSubtitles...");
  const panel = pad.panel;
  let found;
  try { found = await invoke("subs_search", { id: s.tile.jf_id, lang }); } catch (e) {
    if (pad.panel !== panel || session !== s) return;
    if (String(e) === "setup") openList(title, [{ label: "Open Settings", accent: true, run: () => { close(); location.hash = "#settings/account"; } }], back, SETUP_NOTE);
    else openList(title, [], back, String(e));
    return;
  }
  if (pad.panel !== panel || session !== s) return;
  if (!found.length) { openList(title, [], back, "Nothing found in this language."); return; }
  openList(title, found.slice(0, 30).map((f) => ({
    label: f.name,
    meta: resultMeta(f),
    async run(it) {
      it.busy = true;
      const small = it.el?.querySelector("small");
      if (small) small.textContent = "Downloading...";
      try {
        if (await addSub(s, f, lang) && pad.panel) closePanel();
      } catch (err) {
        toast(String(err), true);
        it.busy = false;
        if (small) small.textContent = it.meta;
      }
    },
  })), back);
}

function panelInput(a) {
  const p = pad.panel;
  if (a === "back") { p.back(); return; }
  if (a === "play") { togglePlay(); return; }
  if (a === "seek-back" || a === "seek-fwd") { seekBy(a === "seek-back" ? -10 : 10); return; }
  if (p.kind === "volume") {
    if (a === "left" || a === "down") setVolume(level - 0.05);
    else if (a === "right" || a === "up") setVolume(level + 0.05);
    else if (a === "select") closePanel();
    return;
  }
  if (a === "up") listSel(p.i - 1);
  else if (a === "down") listSel(p.i + 1);
  else if (a === "select") pick();
}

/** One controller button, from nav.js. True when the player used it (always, while it is open). */
export function padInput(a) {
  if (box.hidden) return false;
  if (!pad.on) enterPad();
  if (pad.panel) { panelInput(a); return true; }
  // Shortcuts that work with the overlay up or hidden.
  if (a === "play") { togglePlay(); osd(video.paused ? "Paused" : "Playing"); }
  else if (a === "seek-back") seekBy(-10);
  else if (a === "seek-fwd") seekBy(10);
  else if (a === "tab-prev") cycleSub(-1);
  else if (a === "tab-next") cycleSub(1);
  else if (a === "search") { padWake(); subsList(); return true; }
  else if (shown()) {
    if (a === "left") barSel(pad.i - 1);
    else if (a === "right") barSel(pad.i + 1);
    else if (a === "back" || a === "down") { padHide(); return true; }
    else if (a === "select") { padWake(); pad.items[pad.i]?.run(); return true; }
  } else {
    // With the overlay hidden, the pad's select skips the intro or plays the next episode.
    if (a === "select" && skipNow()) return true;
    if (a === "select" && upnext.classList.contains("on")) { hideUpnext(); playNext(); return true; }
    if (a === "select") { padWake(); return true; }
    if (a === "back") { close(); return true; }
    if (a === "left") seekBy(-10);
    else if (a === "right") seekBy(10);
    else if (a === "up") volumeBy(0.1);
    else if (a === "down") volumeBy(-0.1);
    return true;
  }
  if (shown()) padWake();
  return true;
}

/* ---------- Watch now ---------- */
// Plays a title while it downloads. The player opens at once on a waiting
// screen (the stages come from the backend's watchnow-state events), then
// plays the Jellyfin placeholder the backend returns. Closing the player
// tells the backend where you stopped, which pauses the download.
const WAIT = {
  asking: ["Asking for it...", 0.08],
  searching: ["Looking for a copy...", 0.3],
  starting: ["Starting the download...", 0.55],
  buffering: ["Getting the first minutes...", 0.75],
  ready: ["Getting the first minutes...", 1],
};
const waitBox = $("#p-wait");
/** The Watch now title open in the player: { key, waiting, at, have, timer }. */
let wn = null;

/** A Continue Watching tile from Watch now (its key is Watch now's own). */
export const isWatchNow = (t) => /^(movie|ep):/.test(t?.key || "");

/** A film: a movie, or an anime whose format is MOVIE. Everything else is a show. */
export const movieLike = (t) => t.kind === "movie" || (t.kind === "anime" && t.format === "MOVIE");

/** Whether Watch now can tell which title this is (TMDB for a movie, TVDB for a show). */
export const canWatchNow = (t) => isWatchNow(t) || (movieLike(t) ? !!t.tmdb : !!t.tvdb);

function keyOf(t, season, episode) {
  if (isWatchNow(t)) return t.key;
  if (movieLike(t)) return t.tmdb ? `movie:${t.tmdb}` : null;
  return t.tvdb && season != null ? `ep:${t.tvdb}:${season}:${episode}` : null;
}

function waitTitle(t, season, episode) {
  $("#p-title").textContent = t.title;
  $("#p-sub").textContent = t.subtitle || (season != null && !movieLike(t) ? `S${season} E${episode}` : t.year ? String(t.year) : "");
}

function openWait(t, season, episode) {
  waitTitle(t, season, episode);
  const pic = t.backdrop || t.poster;
  $("#p-wait-bg").style.backgroundImage = pic ? `url("${pic.replace(/"/g, "%22")}")` : "none";
  $("#p-wait-stage").textContent = "";
  stage("asking", 0);
  box.classList.add("waiting");
  waitBox.classList.remove("out");
  waitBox.hidden = false;
  $("#p-spinner").hidden = true;
}

function stage(name, pct) {
  const [text, at] = WAIT[name] || WAIT.asking;
  const line = $("#p-wait-stage");
  if (line.textContent !== text) {
    line.textContent = text;
    line.animate([{ opacity: 0, transform: "translateY(6px)" }, { opacity: 1, transform: "none" }], { duration: 280, easing: "cubic-bezier(0.16, 1, 0.3, 1)" });
  }
  // Getting the first minutes fills the rest of the bar with the download's own progress.
  const p = name === "buffering" ? at + (1 - at) * Math.min(1, (pct || 0) / 100) : at;
  $("#p-wait-fill").style.transform = `scaleX(${p})`;
}

/** The waiting screen fades out (at once when `now`). */
function endWait(now = false) {
  box.classList.remove("waiting");
  if (waitBox.hidden) return;
  if (now) { waitBox.hidden = true; waitBox.classList.remove("out"); return; }
  waitBox.classList.add("out");
  setTimeout(() => { if (waitBox.classList.contains("out")) { waitBox.hidden = true; waitBox.classList.remove("out"); } }, 420);
}

listen("watchnow-state", (e) => {
  const p = e.payload || {};
  if (!wn?.waiting || (wn.key && p.key !== wn.key)) return;
  if (WAIT[p.stage]) stage(p.stage, p.pct);
});

/* Waiting for data past what's downloaded: ask again after 3, 6 and 12 s, then
   every 20 s, with the spinner up. After 3 minutes it gives up and closes
   (which leaves, so where you were is kept and the download pauses). */
const RETRY_STEPS = [3000, 6000, 12000];
const RETRY_EVERY = 20000;
const RETRY_FOR = 180000;

function waitForData(again) {
  const w = wn;
  if (!w) return;
  w.retry ??= { n: 0, since: Date.now(), timer: null };
  const r = w.retry;
  if (r.timer) return;
  $("#p-spinner").hidden = false;
  if (Date.now() - r.since >= RETRY_FOR) {
    w.retry = null;
    toast("Still getting this ready. Try again in a bit.", true);
    close();
    return;
  }
  const ms = RETRY_STEPS[r.n] ?? RETRY_EVERY;
  r.n++;
  r.timer = setTimeout(() => { r.timer = null; if (wn === w && !box.hidden) again(); }, ms);
}

/** The downloaded part (the chosen file's own progress), drawn behind the played part of the seek bar. */
function drawHave() {
  seek.classList.toggle("wn", !!wn);
  seek.style.setProperty("--have", `${Math.round((wn?.have || 0) * 1000) / 10}%`);
}

async function pollHave() {
  const w = wn;
  if (!w || w.waiting) return;
  const list = await invoke("watchnow_list").catch(() => null);
  const t = list?.find((x) => x.key === w.key);
  if (wn !== w || t?.downloaded == null) return;
  w.have = t.downloaded;
  drawHave();
}

/** The player is closing (or playing something else): where you stopped goes back, which pauses the download. */
function leaveWatch() {
  const w = wn;
  if (!w) return;
  wn = null;
  clearInterval(w.timer);
  if (w.retry?.timer) clearTimeout(w.retry.timer);
  drawHave();
  // Still waiting: watchNow() sends it once the backend is done.
  if (w.waiting) return;
  const position = session?.watchKey === w.key && video.readyState >= 1 ? now() : w.at;
  invoke("watchnow_leave", { key: w.key, position, duration: total() || null, uid: w.uid }).catch((e) => toast(String(e), true));
}

/**
 * Watch now: plays `tile` while it downloads. `opts.season` and `opts.episode`
 * pick an episode (the tile is then the show); `opts.start` plays from there
 * instead of where you stopped; `opts.where` forces "vlc" or "chillarr";
 * `opts.resolve` (async, returning { tile, season, episode } or
 * { play: tile }) works out the episode once the waiting screen is up.
 */
export async function watchNow(tile, opts = {}) {
  let { season = null, episode = null } = opts;
  const m = /^ep:\d+:(\d+):(\d+)$/.exec(tile.key || "");
  if (m && season == null) { season = +m[1]; episode = +m[2]; }
  const useVlc = (opts.where || store.settings?.player) === "vlc";
  // An early start of this very episode (Up next) is this press's now.
  dropEarly(keyOf(tile, season, episode));
  leaveWatch();
  if (!box.hidden) close();
  const seq = ++loadSeq;
  box.hidden = false;
  document.body.style.overflow = "hidden";
  poke();
  wn = { key: keyOf(tile, season, episode), waiting: true, at: 0, have: 0, timer: null };
  openWait(tile, season, episode);
  if (opts.resolve) {
    const r = await opts.resolve().catch(() => null);
    if (seq !== loadSeq) return;
    if (!r) { wn = null; toast("Couldn't tell which episode to play.", true); close(false); return; }
    if (r.play) { wn = null; close(false); await play(r.play, r.playlist || null, opts.where); return; }
    ({ tile, season, episode } = r);
    wn.key = keyOf(tile, season, episode);
    waitTitle(tile, season, episode);
  }
  let got;
  try {
    got = await invoke("watchnow_start", { tile, season, episode });
  } catch (e) {
    if (seq !== loadSeq) return;
    // Nothing found (or a plain no): say so and back out. Nothing plays, so nothing to leave.
    wn = null;
    toast(String(e), true);
    close(false);
    return;
  }
  if (seq !== loadSeq) {
    // Closed while it got ready: nobody is watching it after all. Unless the
    // same title was pressed again meanwhile: that newer session is watching it.
    if (wn?.key !== got.key) invoke("watchnow_leave", { key: got.key, position: got.resume_seconds || 0, duration: null, uid: got.profile }).catch(() => {});
    return;
  }
  const at = opts.start ?? got.resume_seconds ?? 0;
  if (useVlc) {
    wn = null;
    endWait(true);
    box.hidden = true;
    document.body.style.overflow = "";
    try {
      await invoke("media_play_vlc", { id: got.jf_id, start: at, title: got.subtitle ? `${got.title} ${got.subtitle}` : got.title, watchKey: got.key });
      toast("Opening in VLC. Chillarr keeps track of where you stop.");
    } catch (e) {
      toast(String(e), true);
      invoke("watchnow_leave", { key: got.key, position: at, duration: null, uid: got.profile }).catch(() => {});
    }
    return;
  }
  Object.assign(wn, { key: got.key, waiting: false, at, uid: got.profile || null });
  drawHave();
  pollHave();
  wn.timer = setInterval(pollHave, 5000);
  await load(got, null, at, { watchKey: got.key });
}

/**
 * The show's next episode after the one in `s` (a library episode or a Watch
 * now one), from the show's full list, so one that isn't here is offered too.
 * One that's here plays from the library; one that isn't plays with Watch now
 * (only that episode downloads). Specials are skipped; one not out yet means
 * no next. Without the list (it couldn't be read) the playlist stands.
 */
async function findNext(s) {
  const m = /^ep:(\d+):(\d+):(\d+)$/.exec(s.watchKey || "");
  if (!m && s.tile.kind !== "episode") return;
  const ask = m ? { key: `tvdb:${m[1]}`, kind: "series", title: s.tile.title, poster: s.tile.poster, backdrop: s.tile.backdrop, tvdb: +m[1] } : s.tile;
  const d = await invoke("media_show_episodes", { tile: ask }).catch(() => null);
  // Another audio or subtitle track reloads into a new session: same episode, so it still counts.
  if (!d || session?.tile.jf_id !== s.tile.jf_id) return;
  s = session;
  const rows = d.episodes || [];
  const i = m ? rows.findIndex((r) => r.season === +m[2] && r.episode === +m[3]) : rows.findIndex((r) => r.jf_id && r.jf_id === s.tile.jf_id);
  if (i < 0 || rows[i].season === 0) return;
  const r = nextRow(rows, i);
  const show = { ...(d.show || ask), tvdb: d.tvdb ?? d.show?.tvdb ?? ask.tvdb ?? null };
  const playlist = rows.filter((x) => x.tile).map((x) => x.tile);
  if (!r) s.next = null;
  else if (r.tile && !s.watchKey) { s.next = r.tile; s.playlist = playlist; }
  // Watch now closes this stream itself; the library copy doesn't, so it's told here.
  else if (r.tile) s.next = { ...r.tile, go: () => { report("stop"); play(r.tile, playlist, "chillarr"); } };
  else if (!show.tvdb) return; // Watch now can't tell which show it is
  else {
    s.next = {
      title: show.title,
      subtitle: `S${r.season} E${r.episode}${r.title ? ` · ${r.title}` : ""}`,
      backdrop: r.still || show.backdrop,
      poster: show.poster,
      fetch: true,
      key: `ep:${show.tvdb}:${r.season}:${r.episode}`,
      show, season: r.season, episode: r.episode,
      // No Watch now copy of it yet, so an early start nobody goes on to is removed again.
      fresh: !r.watch_key,
      go: () => watchNow(show, { season: r.season, episode: r.episode, where: "chillarr" }),
    };
  }
  // Up next was already up with the playlist's pick: it comes back with this one.
  if (upnext.classList.contains("on")) hideUpnext();
  nextButton();
  padRefresh();
}

/** The show's next episode after row `i` (specials sort last, so they end it); null when it isn't out yet. */
function nextRow(rows, i) {
  const r = rows.slice(i + 1).find((x) => x.season > 0);
  return r?.aired ? r : null;
}

/** "S1 E3" out of a next episode's label. */
const epTag = (n) => (/^S\d+ E\d+/.exec(n.subtitle || "") || [""])[0];

/** The Next episode button: hidden without one, else it names it. */
function nextButton() {
  const n = nextEpisode(), b = $("#p-next");
  b.hidden = !n;
  if (!n) return;
  b.textContent = epTag(n) ? `Next: ${epTag(n)}` : "Next episode";
  b.title = `${n.subtitle || n.title}${n.fetch ? ". Downloads as you watch." : ""}`;
}

/* Up next for an episode that isn't here starts its Watch now as the card
   comes up, so it's on its way by the time it plays. If nobody goes on to it
   (the player closed, or something else played) it's undone the way Watch
   now's Remove does it: the entry and its download go. */
let early = null; // { key, run: Promise<tile | null> }

function startEarly(n) {
  if (!n.fresh || early?.key === n.key) return;
  dropEarly();
  early = { key: n.key, run: invoke("watchnow_start", { tile: n.show, season: n.season, episode: n.episode }).catch(() => null) };
}

/** Lets go of the early start, unless it's `keep` (that one is playing now). */
function dropEarly(keep = null) {
  const e = early;
  early = null;
  if (!e || e.key === keep) return;
  e.run.then((got) => {
    // It failed (nothing to undo), or it's being watched after all.
    if (!got || wn?.key === got.key) return;
    invoke("watchnow_remove", { key: got.key }).catch(() => {});
  });
}

/** Watch now on a show: the episode you're at, else its first. One already downloaded just plays. */
export function watchNowShow(t, opts = {}) {
  return watchNow(t, { ...opts, resolve: async () => {
    const d = await invoke("media_show_episodes", { tile: t });
    const rows = d.episodes || [];
    const r = (d.spot && rows.find((x) => x.season === d.spot.season && x.episode === d.spot.episode)) || rows.find((x) => x.season > 0 && x.aired);
    if (!r) return null;
    if (r.tile) return { play: r.tile, playlist: rows.filter((x) => x.tile).map((x) => x.tile) };
    const show = d.show || t;
    return { tile: { ...show, tvdb: d.tvdb ?? show.tvdb }, season: r.season, episode: r.episode };
  } });
}

/** Asks first, with the themed confirm. True on yes. */
export function confirmRemove(t) {
  const name = t.subtitle && !movieLike(t) ? `${t.title} ${t.subtitle}` : t.title;
  return ask(`Remove ${name}?`, "The part that's downloaded is deleted. Chillarr still remembers where you were.", [], "Remove");
}

/** Remove from Watch now: stops it and deletes the partial download. Where you were is kept. */
export async function removeWatchNow(t) {
  if (!(await confirmRemove(t))) return false;
  try {
    toast(await invoke("watchnow_remove", { key: t.key }));
    window.dispatchEvent(new Event("chillarr:refresh"));
    return true;
  } catch (e) {
    toast(String(e), true);
    return false;
  }
}
