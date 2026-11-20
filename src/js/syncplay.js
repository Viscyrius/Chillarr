// Watch together: Jellyfin's SyncPlay, so Chillarr can host or join a watch
// party with anyone on this Jellyfin, in Chillarr or in Jellyfin's own apps.
// Jellyfin is the referee: play, pause and seek become requests, and the
// player only moves when the group's command comes back, at the moment it
// names. Message shapes are the ones measured on Jellyfin 12.1.
//
// Chat: everyone in the party can type to each other while it plays. SyncPlay
// has no messages, so Chillarr carries them (syncplay.rs `say`): the window
// hears a Tauri event, a web page its bridge stream, the same "together-chat".
//
// Host tools (syncplay.rs `Room`): whoever made the party is its host. They
// can lock it or ask for a password, remove someone, and turn on "Only I
// control". The PC holds and enforces all of it; this page only shows it and
// asks. Joining goes through syncplay_join, never Jellyfin's Join directly.
import { $, ask, el, getPref, icon, invoke, listen, setPref, store, toast } from "./api.js";
import { syncControl as player } from "./player.js";

const TICKS = 1e7;
const dlg = $("#together");

let ws = null;
let me = "";
let offset = 0; // the server's clock minus ours, ms
let rtt = 0;
let keepAlive = null, pingTimer = null, driftTimer = null, cmdTimer = null;
let waitingReady = false;
let stallTimer = null; // a "waiting" that hasn't lasted long enough to tell the party yet
// This page asked for a new party (create), so the GroupJoined that follows is its own.
let creating = false;
// Catching up by speed: on until it's really in step, so it doesn't flip on and off.
let easing = false;
const g = { id: null, name: "", people: [], itemId: null, playlistItemId: null, state: "Idle", cmd: null, startWhenReady: false, startTicks: 0, title: "" };
// Join back (#36): this player is opening the party's title to catch up with
// it. Local only: nothing is sent to the party until it can play from there.
let rejoin = null;
// The party's host tools as this profile sees them (syncplay_room), or null
// when Chillarr doesn't know the party (made in a Jellyfin app before it saw it).
let room = null;
// The host removed this profile: say so once, not "You left".
let kicked = false;

export const inGroup = () => !!g.id;
/** "Only I control" is on and this profile isn't the host. */
const controlled = () => !!(g.id && room && room.host_only && !room.host);
const hostHasIt = () => { toast("The host is in control."); };

async function loadRoom() {
  if (!g.id) { room = null; return; }
  const id = g.id;
  const r = await invoke("syncplay_room", { group: id }).catch(() => null);
  if (g.id !== id) return;
  room = r;
  draw();
}

/* ---------- Clock ---------- */
// Jellyfin writes 7 decimal places; Date.parse wants at most 3.
const parse = (iso) => Date.parse(String(iso).replace(/(\.\d{3})\d+/, "$1"));
const serverNow = () => Date.now() + offset;
const isoNow = () => new Date(serverNow()).toISOString();

const call = (path, body = null, method = "POST") => invoke("syncplay_call", { method, path, body });

async function syncClock() {
  let best = null;
  for (let i = 0; i < 3; i++) {
    const t0 = Date.now();
    const r = await call("/GetUtcTime", null, "GET");
    const t3 = Date.now();
    const t1 = parse(r.RequestReceptionTime), t2 = parse(r.ResponseTransmissionTime);
    const s = { off: (t1 - t0 + (t2 - t3)) / 2, rtt: t3 - t0 - (t2 - t1) };
    if (!best || s.rtt < best.rtt) best = s;
  }
  offset = best.off;
  rtt = Math.max(0, best.rtt);
}

/* ---------- Connection ---------- */
/** Tries again until it's back, waiting longer each time (up to a minute),
    while still in a party: one failed try used to end the retries for good,
    and the party went quiet while still showing as joined. */
function reconnect(wait) {
  setTimeout(() => {
    if (!g.id || (ws && ws.readyState <= 1)) return;
    connect().catch(() => reconnect(Math.min(60000, wait * 2)));
  }, wait);
}

async function connect() {
  if (ws && ws.readyState <= 1) return;
  const info = await invoke("syncplay_info");
  me = info.user;
  await new Promise((ok, bad) => {
    ws = new WebSocket(info.socket);
    ws.onopen = ok;
    ws.onerror = () => bad("Couldn't reach Jellyfin for Watch together.");
    ws.onmessage = (e) => { try { onMessage(JSON.parse(e.data)); } catch (err) { /* not for us */ } };
    ws.onclose = () => {
      ws = null;
      clearInterval(keepAlive);
      // Still in a group: Jellyfin restarted or the PC slept. Come back.
      if (g.id) reconnect(3000);
    };
  });
  // A full client, as Jellyfin's own apps say they are.
  await call("/Sessions/Capabilities/Full", { PlayableMediaTypes: ["Video"], SupportedCommands: [], SupportsMediaControl: true }).catch(() => {});
  await syncClock().catch(() => {});
}

function onMessage(m) {
  if (m.MessageType === "ForceKeepAlive") {
    const send = () => ws?.readyState === 1 && ws.send(JSON.stringify({ MessageType: "KeepAlive" }));
    send();
    clearInterval(keepAlive);
    keepAlive = setInterval(send, Math.max(5, (m.Data || 60) / 2) * 1000);
  } else if (m.MessageType === "SyncPlayGroupUpdate") groupUpdate(m.Data);
  else if (m.MessageType === "SyncPlayCommand") command(m.Data);
}

/* ---------- Group ---------- */
function groupUpdate(u) {
  if (g.id && u.GroupId && u.GroupId !== g.id && u.Type !== "GroupJoined") return;
  switch (u.Type) {
    case "GroupJoined":
      Object.assign(g, { id: u.GroupId, name: u.Data.GroupName, people: u.Data.Participants || [], state: u.Data.State || "Idle" });
      kicked = false;
      startTimers();
      loadRoom();
      toast(`You're in ${g.name}.`);
      // Started while watching something: bring the group to it.
      // Started a party while watching something: bring the group to it. Only
      // a party made here: joining a friend's used to switch THEIR party to
      // whatever was open on this screen.
      if (creating && player.currentTile()) startedHere(player.currentTile(), player.now(), !player.paused());
      creating = false;
      break;
    case "UserJoined":
      if (!g.people.includes(u.Data)) g.people.push(u.Data);
      notice(`${u.Data} joined the watch party.`);
      // The PC checks they came in through Chillarr while it's locked.
      invoke("syncplay_seen", { group: g.id, name: u.Data }).catch(() => {});
      break;
    case "UserLeft":
      g.people = g.people.filter((p) => p !== u.Data);
      notice(`${u.Data} left the watch party.`);
      invoke("syncplay_gone", { group: g.id, name: u.Data }).catch(() => {});
      break;
    case "GroupLeft":
    case "NotInGroup":
    case "GroupDoesNotExist":
      leftGroup();
      break;
    case "PlayQueue":
      playQueue(u.Data);
      break;
    case "StateUpdate":
      g.state = u.Data.State;
      break;
    case "LibraryAccessDenied":
      notice("Someone in the party can't see this in their Jellyfin library.", true);
      break;
  }
  draw();
}

async function playQueue(d) {
  const item = d.Playlist?.[d.PlayingItemIndex];
  if (!item) return;
  if (g.itemId !== item.ItemId) g.title = "";
  g.playlistItemId = item.PlaylistItemId;
  g.itemId = item.ItemId;
  g.startTicks = d.StartPositionTicks || 0;
  if (player.currentId() === item.ItemId) { await ready(); return; }
  const tile = await invoke("syncplay_tile", { id: item.ItemId }).catch(() => null);
  if (!tile) { toast("The party is playing something this profile can't see.", true); return; }
  g.title = nameOf(tile);
  toast(`The party is watching ${g.title}.`);
  player.open(tile, (d.StartPositionTicks || 0) / TICKS);
}

function command(c) {
  if (!g.id || c.GroupId !== g.id) return;
  g.cmd = c;
  clearTimeout(cmdTimer);
  const pos = c.PositionTicks / TICKS;
  const at = parse(c.When) - offset;
  if (c.Command === "Unpause") {
    player.rate(1);
    // Only a real gap is jumped: any jump makes the player wait a moment, and a
    // jump to where it already is used to be reported as buffering, which the
    // party answered with another Unpause (1,700 a minute, 2026-11-06: the
    // micro stutters). Smaller gaps are caught up by speed (driftTimer).
    const go = () => { seekIfOff(pos + Math.max(0, Date.now() - at) / 1000); player.play(); };
    const wait = at - Date.now();
    if (wait > 0) cmdTimer = setTimeout(go, wait); else go();
  } else if (c.Command === "Pause") {
    player.pause();
    seekIfOff(pos);
  } else if (c.Command === "Seek") {
    // The party jumped while this one was joining back: the normal path takes over.
    rejoin = null;
    player.pause();
    player.seek(pos);
    whenPlayable(() => ready());
  } else if (c.Command === "Stop") {
    if (g.itemId && player.currentId()) player.pause();
  }
}

const nameOf = (tile) => (tile.subtitle ? `${tile.title} ${tile.subtitle}` : tile.title);

/** Where the party is now, in seconds: its last command's spot plus the time
 *  since when it's playing (the drift check's sum). */
function partyAt() {
  const c = g.cmd;
  if (!c) return g.startTicks / TICKS;
  const pos = c.PositionTicks / TICKS;
  return c.Command === "Unpause" ? pos + Math.max(0, serverNow() - parse(c.When)) / 1000 : pos;
}
const partyPlaying = () => g.cmd?.Command === "Unpause";

/** Join back is offered: in a party on a title, and this player isn't on it. */
const canJoinBack = () => !!g.id && !!g.itemId && g.state !== "Idle" && player.currentId() !== g.itemId;

/** Opens the party's title at the party's spot, in its play or pause state.
 *  Nobody else is told anything: no Pause, Seek or Unpause goes out. */
async function joinBack() {
  // A rejoin that never got going (the player shut while loading) stops blocking.
  if (rejoin && Date.now() - rejoin.at > 30000) rejoin = null;
  if (rejoin || !canJoinBack()) return;
  const item = g.itemId;
  rejoin = { item, at: Date.now() };
  draw();
  const tile = await invoke("syncplay_tile", { id: item }).catch(() => null);
  if (!tile) {
    rejoin = null;
    toast("This profile can't open what the party is watching. It may be in another profile's library, or above this profile's age limit.", true);
    draw();
    return;
  }
  if (!g.id || g.itemId !== item) { rejoin = null; draw(); return; }
  g.title = nameOf(tile);
  if (dlg.open) dlg.close();
  player.open(tile, partyAt());
}

/** The rejoining player can play: catch up to where the party got to while it
 *  loaded, match its play or pause, then say Ready (Jellyfin answers only this
 *  device), and the normal drift check takes over. */
function rejoined() {
  const r = rejoin;
  if (!g.id || g.itemId !== r.item || player.currentId() !== r.item) { rejoin = null; ready(); return; }
  seekIfOff(partyAt());
  whenPlayable(() => {
    rejoin = null;
    if (partyPlaying()) { seekIfOff(partyAt()); player.play(); } else player.pause();
    ready();
    draw();
  });
}

/** The player closed on the party's title: say how to get back in. */
export function leftPlayer(id) {
  if (rejoin) { rejoin = null; draw(); return; }
  if (!g.id || id !== g.itemId) return;
  toast("You left the movie. Join back any time from Watch together.");
  draw();
}

const clock = (t) => {
  const s = Math.max(0, Math.floor(t));
  const h = Math.floor(s / 3600), m = Math.floor((s % 3600) / 60), x = s % 60;
  const two = (n) => (n < 10 ? `0${n}` : String(n));
  return h ? `${h}:${two(m)}:${two(x)}` : `${m}:${two(x)}`;
};

/** Jumps only when this player is a second or more away from `pos`. */
function seekIfOff(pos) {
  if (Math.abs(player.now() - pos) >= 1) player.seek(pos);
}

// One wait at a time: a second Seek or Pause before the first could play
// used to leave both listeners, and each sent its own Ready.
let playable = null;
function whenPlayable(fn) {
  const v = player.video;
  if (playable) { playable.v.removeEventListener("canplay", playable.go); playable = null; }
  if (v.readyState >= 3) { fn(); return; }
  const go = () => { playable = null; fn(); };
  playable = { v, go };
  v.addEventListener("canplay", go, { once: true });
}

/** Tells the group this player can play from where it is. */
async function ready() {
  if (!g.id || !g.playlistItemId) return;
  await call("/SyncPlay/Ready", { When: isoNow(), PositionTicks: Math.round(player.now() * TICKS), IsPlaying: !player.paused(), PlaylistItemId: g.playlistItemId }).catch(() => {});
  if (g.startWhenReady) { g.startWhenReady = false; requestPlay(); }
}

function startTimers() {
  clearInterval(pingTimer);
  clearInterval(driftTimer);
  pingTimer = setInterval(() => { if (g.id) call("/SyncPlay/Ping", { Ping: Math.round(rtt) }).catch(() => {}); }, 10000);
  // Keep in step, gently: it takes its time rather than stutter. Under half a
  // second off is left alone; more is caught up by playing 3% faster or
  // slower (not heard), held until it's within 0.15 s so the speed doesn't
  // flip back and forth; only 3 seconds or more is a jump.
  driftTimer = setInterval(() => {
    const c = g.cmd;
    if (!g.id || !c || c.Command !== "Unpause" || player.paused() || waitingReady) { easing = false; player.rate(1); return; }
    const expected = c.PositionTicks / TICKS + (serverNow() - parse(c.When)) / 1000;
    const diff = player.now() - expected;
    const off = Math.abs(diff);
    if (off >= 3) { easing = false; player.rate(1); player.seek(expected); }
    else if (off > 0.5 || (easing && off > 0.15)) { easing = true; player.rate(diff > 0 ? 0.97 : 1.03); }
    else { easing = false; player.rate(1); }
  }, 1000);
}

function leftGroup() {
  const was = g.name;
  room = null;
  chatReset();
  Object.assign(g, { id: null, name: "", people: [], itemId: null, playlistItemId: null, state: "Idle", cmd: null, startWhenReady: false, startTicks: 0, title: "" });
  rejoin = null;
  clearInterval(pingTimer);
  clearInterval(driftTimer);
  clearTimeout(cmdTimer);
  clearTimeout(stallTimer);
  stallTimer = null;
  waitingReady = false;
  easing = false;
  player.rate(1);
  if (was) toast(kicked ? "You were removed from the party." : `You left ${was}.`, kicked);
  kicked = false;
  draw();
}

/* ---------- What the player asks for while in a party ---------- */
// With "Only I control" on, nothing is sent for anyone but the host (the PC
// refuses them too).
export function requestPlay() { if (controlled()) { hostHasIt(); return; } call("/SyncPlay/Unpause").catch((e) => toast(String(e), true)); }
export function requestPause() { if (controlled()) { hostHasIt(); return; } call("/SyncPlay/Pause").catch((e) => toast(String(e), true)); }
export function requestSeek(seconds) { if (controlled()) { hostHasIt(); return; } call("/SyncPlay/Seek", { PositionTicks: Math.round(Math.max(0, seconds) * TICKS) }).catch((e) => toast(String(e), true)); }

/** Something was started in this player: the whole party switches to it. */
export function startedHere(tile, at, playing = true) {
  if (!g.id || !tile?.jf_id || tile.jf_id === g.itemId) return;
  if (controlled()) { hostHasIt(); return; }
  g.itemId = tile.jf_id;
  g.startWhenReady = playing;
  call("/SyncPlay/SetNewQueue", { PlayingQueue: [tile.jf_id], PlayingItemPosition: 0, StartPositionTicks: Math.round((at || 0) * TICKS) }).catch((e) => toast(String(e), true));
}

/** The party's title finished loading here: say so once it can play. */
export function loadedHere() { whenPlayable(() => (rejoin ? rejoined() : ready())); }

/** The player stalled or caught up: the party waits for it, then goes on. */
// Only a wait that lasts is told: the browser says "waiting" for a few
// milliseconds after every jump, and each Buffering makes the whole party stop.
const STALL_MS = 600;
// `now`: a reload for another track, which is paused while it loads and needs
// the party's Unpause to go on, so it can't wait to see if it lasts.
export function buffering(now = false) {
  if (!g.id || !g.playlistItemId || waitingReady || (stallTimer && !now)) return;
  const tell = () => {
    clearTimeout(stallTimer);
    stallTimer = null;
    waitingReady = true;
    call("/SyncPlay/Buffering", { When: isoNow(), PositionTicks: Math.round(player.now() * TICKS), IsPlaying: true, PlaylistItemId: g.playlistItemId }).catch(() => {});
  };
  if (now) { tell(); return; }
  stallTimer = setTimeout(() => {
    stallTimer = null;
    const v = player.video;
    if (!g.id || !g.playlistItemId || waitingReady || v.paused || v.readyState >= 3) return;
    tell();
  }, STALL_MS);
}
export function caughtUp() {
  clearTimeout(stallTimer);
  stallTimer = null;
  if (!waitingReady) return;
  waitingReady = false;
  ready();
}

/* ---------- Panel ---------- */
async function create() {
  try {
    await connect();
    creating = true;
    await call("/SyncPlay/New", { GroupName: `${me}'s watch party` });
  } catch (e) { creating = false; toast(String(e), true); }
}

/** Joins through the PC, which checks the host's lock and password first. */
async function join(id, password = null) {
  try {
    creating = false;
    await connect();
    await invoke("syncplay_join", { group: id, password });
    return true;
  } catch (e) { toast(String(e), true); return false; }
}

async function leave() {
  await call("/SyncPlay/Leave").catch(() => {});
  leftGroup();
}

async function draw() {
  for (const b of [$("#together-btn"), $("#p-together")]) b.classList.toggle("on", !!g.id);
  chatBtn.hidden = !g.id;
  $("#player").classList.toggle("wt-follow", controlled());
  if (!dlg.open) return;
  const head = el("h2", { text: g.id ? g.name : "Watch together" });
  const parts = [head];
  if (g.id) {
    parts.push(
      el("div", { class: "wt-people" }, g.people.map((p) => el("span", { class: "wt-person" }, el("span", { class: "wt-dot" }), p))),
      el("p", { class: "wt-note", text: controlled() ? "The host is in control. Volume, subtitles and chat are still yours." : g.itemId ? "Play, pause and skips happen for everyone. If someone's connection is slow, the party waits for them." : "Pick something to watch: everyone in the party starts it together." }),
      hostTools(),
      joinBackButton(),
      el("p", { class: "wt-note", text: "To chat with the party, press C in the player or use the chat button at the top." }),
      invite(),
      el("div", { class: "ask-actions" },
        el("button", { class: "btn ghost", onclick: leave }, "Leave party"),
        el("button", { class: "btn primary", onclick: () => dlg.close() }, "Done")));
  } else {
    parts.push(el("p", { class: "wt-note", text: "Watch in sync with friends: everyone sees the same moment, and play, pause and skip happen for all." }));
    const list = el("div", { class: "wt-groups" }, el("p", { class: "wt-note", text: "Looking for parties..." }));
    parts.push(el("div", { class: "ask-actions", style: "justify-content:flex-start" }, el("button", { class: "btn primary", onclick: create }, "Start a watch party")),
      el("h3", { text: "Join one" }), list, invite(),
      el("div", { class: "ask-actions" }, el("button", { class: "btn ghost", onclick: () => dlg.close() }, "Close")));
    Promise.all([call("/SyncPlay/List", null, "GET"), invoke("syncplay_locks").catch(() => ({}))]).then(([groups, locks]) => {
      const rows = (groups || []).map((x) => groupRow(x, (locks || {})[x.GroupId] || "open"));
      list.replaceChildren(...(rows.length ? rows : [el("p", { class: "wt-note", text: "No parties right now. Start one, or ask a friend to." })]));
    }).catch((e) => list.replaceChildren(el("p", { class: "wt-note", text: String(e) })));
  }
  dlg.replaceChildren(...parts);
}

const LOCK_ICON = "M7 11V7a5 5 0 0 1 10 0v4M5 11h14v10H5z";

/** One party in the list: Join, a password box first, or why it's shut. */
function groupRow(x, lock) {
  const name = el("b", { text: x.GroupName });
  if (lock !== "open") name.prepend(el("span", { class: "wt-lock", title: lock === "locked" ? "Locked" : "Needs a password" }, icon(LOCK_ICON)));
  const info = el("span", {}, name, el("small", { text: lock === "locked" ? "Locked. Ask the host to open it." : x.Participants.join(", ") }));
  const row = el("div", { class: "wt-group" }, info);
  if (lock === "locked") return row;
  if (lock === "open") { row.append(el("button", { class: "btn small primary", onclick: () => join(x.GroupId) }, "Join")); return row; }
  const pass = el("input", { type: "password", class: "wt-pass", maxlength: "32", placeholder: "Password", "aria-label": `Password for ${x.GroupName}`, autocomplete: "off" });
  const go = async () => { if (!pass.value) { pass.focus(); return; } if (await join(x.GroupId, pass.value)) pass.value = ""; else { pass.select(); } };
  pass.addEventListener("keydown", (e) => { e.stopPropagation(); if (e.key === "Enter" || e.keyCode === 13) { e.preventDefault(); go(); } });
  row.append(el("span", { class: "wt-pass-row" }, pass, el("button", { class: "btn small primary", onclick: go }, "Join")));
  return row;
}

/** Who hosts, and for the host: lock, "Only I control", Kick, Make host, Removed. */
function hostTools() {
  if (!room) return null;
  const set = (args) => invoke("syncplay_set", Object.assign({ group: g.id }, args)).then((r) => { room = r; draw(); }).catch((e) => toast(String(e), true));
  if (!room.host) {
    return el("div", { class: "wt-host" },
      el("p", { class: "wt-note", text: `${room.host_name || "Someone"} is the host.${room.host_only ? " The host is in control." : ""}${room.lock === "locked" ? " The party is locked." : room.lock === "password" ? " The party needs a password." : ""}` }));
  }
  const parts = [el("h3", { text: room.host_only ? "You're in control" : "You're the host" })];
  // Lock party: Open, Password or Locked.
  const pass = el("input", { type: "password", class: "wt-pass", maxlength: "32", placeholder: room.lock === "password" ? "New password (optional)" : "Password, 4 to 32 characters", "aria-label": "Party password", autocomplete: "new-password", hidden: room.lock !== "password" });
  pass.addEventListener("keydown", (e) => { e.stopPropagation(); if (e.key === "Enter" || e.keyCode === 13) { e.preventDefault(); savePass(); } });
  const savePass = () => set({ lock: "password", password: pass.value });
  const save = el("button", { class: "btn small primary", onclick: savePass, hidden: room.lock !== "password" }, "Save password");
  const seg = el("div", { class: "segmented", role: "group", "aria-label": "Lock party" }, ...[["open", "Open"], ["password", "Password"], ["locked", "Locked"]].map(([v, label]) =>
    el("button", { class: room.lock === v ? "on" : "", "aria-pressed": String(room.lock === v), onclick: () => {
      if (v === "password") { pass.hidden = false; save.hidden = false; pass.focus(); return; }
      set({ lock: v });
    } }, label)));
  parts.push(el("div", { class: "wt-row" }, el("span", { text: "Lock party" }), seg), el("div", { class: "wt-row wt-pass-row" }, pass, save));
  // Only I control.
  const sw = el("input", { type: "checkbox", class: "switch", "aria-label": "Only I control" });
  sw.checked = !!room.host_only;
  sw.addEventListener("change", () => set({ hostOnly: sw.checked }));
  sw.addEventListener("keydown", (e) => { if (e.key === "Enter" || e.keyCode === 13) { e.preventDefault(); sw.click(); } });
  parts.push(el("label", { class: "wt-row" }, el("span", { text: "Only I control" }), sw));
  // People: Kick and Make host.
  const others = room.people.filter((p) => !p.host);
  if (others.length) {
    parts.push(el("div", { class: "wt-list" }, ...others.map((p) => el("div", { class: "wt-group" },
      el("span", {}, el("b", { text: p.name })),
      el("span", { class: "wt-acts" },
        el("button", { class: "btn small ghost", onclick: () => invoke("syncplay_host", { group: g.id, who: p.id }).then((r) => { room = r; draw(); }).catch((e) => toast(String(e), true)) }, "Make host"),
        el("button", { class: "btn small ghost", onclick: async () => {
          if (!(await ask(`Remove ${p.name} from the party?`, "They can't come back for 10 minutes unless you let them.", [], "Remove", "Cancel"))) return;
          invoke("syncplay_kick", { group: g.id, who: p.id }).then((r) => { room = r; draw(); }).catch((e) => toast(String(e), true));
        } }, "Kick"))))));
  }
  if (room.removed.length) {
    parts.push(el("h3", { text: "Removed" }), el("div", { class: "wt-list" }, ...room.removed.map((p) => el("div", { class: "wt-group" },
      el("span", {}, el("b", { text: p.name }), el("small", { text: `Can come back in ${p.mins} min` })),
      el("button", { class: "btn small ghost", onclick: () => invoke("syncplay_allow", { group: g.id, who: p.id }).then((r) => { room = r; draw(); }).catch((e) => toast(String(e), true)) }, "Allow back")))));
  }
  return el("div", { class: "wt-host" }, ...parts);
}

/** "Join back: Title, 1:12:40", the time kept current while the panel is open. */
let joinTick = null;
function joinBackButton() {
  clearInterval(joinTick);
  if (!canJoinBack()) return null;
  const label = el("span");
  const paint = () => {
    const name = g.title ? `${g.title}, ` : "";
    label.textContent = rejoin ? "Joining back..." : `Join back: ${name}${clock(partyAt())}${partyPlaying() ? "" : " (paused)"}`;
  };
  const b = el("button", { class: "btn primary wt-rejoin", disabled: !!rejoin, onclick: joinBack }, icon("M8 5v14l11-7z"), label);
  paint();
  joinTick = setInterval(() => { if (!dlg.open || !b.isConnected) { clearInterval(joinTick); return; } paint(); }, 1000);
  // Its name, for the button, the first time the panel is opened after a join.
  if (!g.title && g.itemId) {
    const item = g.itemId;
    invoke("syncplay_tile", { id: item }).then((t) => { if (t && g.itemId === item) { g.title = nameOf(t); paint(); } }).catch(() => {});
  }
  return el("div", { class: "ask-actions wt-rejoin-row", style: "justify-content:flex-start" }, b);
}

/** How a friend gets in: their own profile, from Chillarr or Jellyfin's apps. */
function invite() {
  const m = store.media || {};
  const addr = m.tailscale || m.address;
  return el("details", { class: "wt-invite" },
    el("summary", { text: "How friends join" }),
    el("ol", {},
      el("li", { text: "Make them a profile here (the avatar at the top, then Add profile) and give it a password for Jellyfin apps." }),
      el("li", { text: addr ? `On their phone, TV or computer, open the Jellyfin app (or a browser) and connect to ${addr}.` : "On their phone, TV or computer, open the Jellyfin app and connect to this PC (Settings > Account & devices shows the address)." }),
      el("li", { text: "Sign in with the profile's name and that password, open the menu, choose SyncPlay, and join your party." })));
}

export function openTogether() {
  if (!dlg.open) dlg.showModal();
  draw();
}

$("#together-btn").addEventListener("click", openTogether);

/* ---------- Chat ---------- */
// A small panel over the picture (C or the chat button) and, while it's shut,
// each new message floats up for a few seconds and fades. Nothing pauses.
const CHAT_MAX = 300;
const FLOAT_MS = 6000;
const said = [];
let unread = 0;
const chatBtn = el("button", { class: "p-btn chat-btn", id: "p-chat-btn", "aria-label": "Party chat", title: "Party chat (C)", hidden: true },
  icon("M4 5h16v11H9l-5 4z"), el("span", { class: "chat-badge", hidden: true }));
$("#p-together").before(chatBtn);
const chatList = el("ol", { class: "chat-list", "aria-live": "polite" });
const chatInput = el("input", { type: "text", class: "chat-input", maxlength: String(CHAT_MAX), placeholder: "Say something", "aria-label": "Message to the party", autocomplete: "off" });
const chatSend = el("button", { class: "btn small primary", type: "submit" }, "Send");
const chatForm = el("form", { class: "chat-form" }, chatInput, chatSend);
// "Show messages while watching": off, nothing comes up over the picture.
// Messages still arrive and wait in the chat, counted on the chat button.
// Kept per profile on this device; it only changes this screen.
const QUIET_PREF = "chillarr-chat-float";
const quiet = () => getPref(QUIET_PREF) === "0";
const quietSw = el("input", { type: "checkbox", class: "switch", "aria-label": "Show messages while watching" });
const chatQuiet = el("label", { class: "chat-quiet" }, quietSw, el("span", { text: "Show messages while watching" }));
const chatPanel = el("div", { class: "p-chat", role: "dialog", "aria-label": "Party chat", hidden: true },
  el("div", { class: "chat-head" }, el("b", { text: "Party chat" }), el("button", { class: "link-btn", type: "button", onclick: () => toggleChat(false) }, "Hide")),
  chatList, chatForm, chatQuiet);
const chatFloat = el("ol", { class: "chat-float", "aria-hidden": "true" });
$("#player").append(chatPanel, chatFloat);
chatBtn.addEventListener("click", (e) => { e.stopPropagation(); toggleChat(); });
chatPanel.addEventListener("click", (e) => e.stopPropagation());

quietSw.addEventListener("change", () => {
  setPref(QUIET_PREF, quietSw.checked ? "1" : "0");
  if (!quietSw.checked) chatFloat.replaceChildren();
});
// Its keys are its own, not the player's (Space would pause). OK/Enter on a
// TV remote flips it, Esc or Back shuts the chat.
quietSw.addEventListener("keydown", (e) => {
  e.stopPropagation();
  if (e.key === "Enter" || e.keyCode === 13) { e.preventDefault(); quietSw.click(); }
  else if (e.key === "Escape" || [461, 10009].includes(e.keyCode)) { e.preventDefault(); toggleChat(false); }
});

const watching = () => !$("#player").hidden;
/** Party news (joined, left...): always in the chat log; over the picture only
 *  when messages may show there. */
function notice(text, bad = false) {
  if (g.id) {
    chatList.append(el("li", { class: "chat-line event" }, el("span", { text })));
    if (chatList.children.length > 400) chatList.firstChild.remove();
    if (!chatPanel.hidden) chatList.scrollTop = chatList.scrollHeight;
  }
  if (!(quiet() && watching())) toast(text, bad);
}

const timeOf = (at) => new Date(at).toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
const line = (m) => el("li", { class: m.from === me ? "chat-line mine" : "chat-line" },
  el("b", { text: m.from }), el("span", { text: m.text }), el("time", { text: timeOf(m.at) }));

function drawBadge() {
  const b = chatBtn.querySelector(".chat-badge");
  b.hidden = !unread;
  b.textContent = unread > 9 ? "9+" : String(unread);
}

/** Opens or shuts the chat panel (no argument: the other way). Only in a party. */
export function toggleChat(on = chatPanel.hidden) {
  if (on && !g.id) return;
  chatPanel.hidden = !on;
  chatFloat.hidden = on;
  if (on) {
    quietSw.checked = !quiet();
    unread = 0;
    drawBadge();
    chatFloat.replaceChildren();
    chatList.scrollTop = chatList.scrollHeight;
    chatInput.focus();
  } else if (chatPanel.contains(document.activeElement)) document.activeElement.blur();
}

function chatReset() {
  said.length = 0;
  unread = 0;
  chatList.replaceChildren();
  chatFloat.replaceChildren();
  toggleChat(false);
  drawBadge();
}

listen("together-chat", (e) => {
  const m = e.payload;
  if (!m || !g.id || m.group !== g.id || said.some((x) => x.id === m.id)) return;
  said.push(m);
  if (said.length > 200) said.shift();
  if (chatList.children.length > 400) chatList.firstChild.remove();
  const near = chatList.scrollHeight - chatList.scrollTop - chatList.clientHeight < 40;
  chatList.append(line(m));
  if (near || m.from === me) chatList.scrollTop = chatList.scrollHeight;
  if (!chatPanel.hidden || m.from === me) return;
  unread++;
  drawBadge();
  if (quiet()) return;
  // While it's shut: the message floats up over the picture, then fades.
  const f = line(m);
  chatFloat.append(f);
  while (chatFloat.children.length > 3) chatFloat.firstChild.remove();
  setTimeout(() => { f.classList.add("out"); setTimeout(() => f.remove(), 600); }, FLOAT_MS);
});

chatForm.addEventListener("submit", async (e) => {
  e.preventDefault();
  const text = chatInput.value.trim();
  if (!text || !g.id) return;
  chatSend.disabled = true;
  try {
    await invoke("syncplay_say", { group: g.id, text });
    chatInput.value = "";
  } catch (err) { toast(String(err), true); }
  chatSend.disabled = false;
  chatInput.focus();
});
// Typing is for the message, not the player's keys (Space, M, F...). Esc
// shuts the panel and leaves the player where it is.
chatInput.addEventListener("keydown", (e) => {
  e.stopPropagation();
  if (e.key === "Escape" || [461, 10009].includes(e.keyCode)) { e.preventDefault(); toggleChat(false); }
});
dlg.addEventListener("click", (e) => { if (e.target === dlg) dlg.close(); });

// The host changed something, or someone came or went: ask again.
listen("together-room", (e) => { if (g.id && e.payload && e.payload.group === g.id) loadRoom(); });
// The host removed this profile: leave the party, and the player plays on, alone.
listen("together-kicked", (e) => {
  if (!g.id || !e.payload || e.payload.group !== g.id) return;
  kicked = true;
  call("/SyncPlay/Leave").catch(() => {});
  leftGroup();
});
