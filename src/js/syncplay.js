// Watch together: Jellyfin's SyncPlay, so Chillarr can host or join a watch
// party with anyone on this Jellyfin, in Chillarr or in Jellyfin's own apps.
// Jellyfin is the referee: play, pause and seek become requests, and the
// player only moves when the group's command comes back, at the moment it
// names. Message shapes are the ones measured on Jellyfin 12.1.
import { $, el, invoke, store, toast } from "./api.js";
import { syncControl as player } from "./player.js";

const TICKS = 1e7;
const dlg = $("#together");

let ws = null;
let me = "";
let offset = 0; // the server's clock minus ours, ms
let rtt = 0;
let keepAlive = null, pingTimer = null, driftTimer = null, cmdTimer = null;
let waitingReady = false;
const g = { id: null, name: "", people: [], itemId: null, playlistItemId: null, state: "Idle", cmd: null, startWhenReady: false };

export const inGroup = () => !!g.id;

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
      if (g.id) setTimeout(() => connect().catch(() => {}), 3000);
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
      startTimers();
      toast(`You're in ${g.name}.`);
      // Started while watching something: bring the group to it.
      if (player.currentTile()) startedHere(player.currentTile(), player.now(), !player.paused());
      break;
    case "UserJoined":
      if (!g.people.includes(u.Data)) g.people.push(u.Data);
      toast(`${u.Data} joined the watch party.`);
      break;
    case "UserLeft":
      g.people = g.people.filter((p) => p !== u.Data);
      toast(`${u.Data} left the watch party.`);
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
      toast("Someone in the party can't see this in their Jellyfin library.", true);
      break;
  }
  draw();
}

async function playQueue(d) {
  const item = d.Playlist?.[d.PlayingItemIndex];
  if (!item) return;
  g.playlistItemId = item.PlaylistItemId;
  g.itemId = item.ItemId;
  if (player.currentId() === item.ItemId) { await ready(); return; }
  const tile = await invoke("syncplay_tile", { id: item.ItemId }).catch(() => null);
  if (!tile) { toast("The party is playing something this profile can't see.", true); return; }
  toast(`The party is watching ${tile.subtitle ? `${tile.title} ${tile.subtitle}` : tile.title}.`);
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
    const go = () => { player.seek(pos + Math.max(0, Date.now() - at) / 1000); player.play(); };
    const wait = at - Date.now();
    if (wait > 0) cmdTimer = setTimeout(go, wait); else go();
  } else if (c.Command === "Pause") {
    player.pause();
    player.seek(pos);
  } else if (c.Command === "Seek") {
    player.pause();
    player.seek(pos);
    whenPlayable(() => ready());
  } else if (c.Command === "Stop") {
    if (g.itemId && player.currentId()) player.pause();
  }
}

function whenPlayable(fn) {
  const v = player.video;
  if (v.readyState >= 3) { fn(); return; }
  v.addEventListener("canplay", fn, { once: true });
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
  // Keep in step: a small difference is caught up by playing 5% faster or
  // slower, a big one by jumping.
  driftTimer = setInterval(() => {
    const c = g.cmd;
    if (!g.id || !c || c.Command !== "Unpause" || player.paused() || waitingReady) { player.rate(1); return; }
    const expected = c.PositionTicks / TICKS + (serverNow() - parse(c.When)) / 1000;
    const diff = player.now() - expected;
    if (Math.abs(diff) > 2) { player.rate(1); player.seek(expected); }
    else if (Math.abs(diff) > 0.3) player.rate(diff > 0 ? 0.95 : 1.05);
    else player.rate(1);
  }, 1000);
}

function leftGroup() {
  const was = g.name;
  Object.assign(g, { id: null, name: "", people: [], itemId: null, playlistItemId: null, state: "Idle", cmd: null, startWhenReady: false });
  clearInterval(pingTimer);
  clearInterval(driftTimer);
  clearTimeout(cmdTimer);
  player.rate(1);
  if (was) toast(`You left ${was}.`);
  draw();
}

/* ---------- What the player asks for while in a party ---------- */
export function requestPlay() { call("/SyncPlay/Unpause").catch((e) => toast(String(e), true)); }
export function requestPause() { call("/SyncPlay/Pause").catch((e) => toast(String(e), true)); }
export function requestSeek(seconds) { call("/SyncPlay/Seek", { PositionTicks: Math.round(Math.max(0, seconds) * TICKS) }).catch((e) => toast(String(e), true)); }

/** Something was started in this player: the whole party switches to it. */
export function startedHere(tile, at, playing = true) {
  if (!g.id || !tile?.jf_id || tile.jf_id === g.itemId) return;
  g.itemId = tile.jf_id;
  g.startWhenReady = playing;
  call("/SyncPlay/SetNewQueue", { PlayingQueue: [tile.jf_id], PlayingItemPosition: 0, StartPositionTicks: Math.round((at || 0) * TICKS) }).catch((e) => toast(String(e), true));
}

/** The party's title finished loading here: say so once it can play. */
export function loadedHere() { whenPlayable(() => ready()); }

/** The player stalled or caught up: the party waits for it, then goes on. */
export function buffering() {
  if (!g.id || !g.playlistItemId || waitingReady) return;
  waitingReady = true;
  call("/SyncPlay/Buffering", { When: isoNow(), PositionTicks: Math.round(player.now() * TICKS), IsPlaying: true, PlaylistItemId: g.playlistItemId }).catch(() => {});
}
export function caughtUp() {
  if (!waitingReady) return;
  waitingReady = false;
  ready();
}

/* ---------- Panel ---------- */
async function create() {
  try {
    await connect();
    await call("/SyncPlay/New", { GroupName: `${me}'s watch party` });
  } catch (e) { toast(String(e), true); }
}

async function join(id) {
  try {
    await connect();
    await call("/SyncPlay/Join", { GroupId: id });
  } catch (e) { toast(String(e), true); }
}

async function leave() {
  await call("/SyncPlay/Leave").catch(() => {});
  leftGroup();
}

async function draw() {
  for (const b of [$("#together-btn"), $("#p-together")]) b.classList.toggle("on", !!g.id);
  if (!dlg.open) return;
  const head = el("h2", { text: g.id ? g.name : "Watch together" });
  const parts = [head];
  if (g.id) {
    parts.push(
      el("div", { class: "wt-people" }, g.people.map((p) => el("span", { class: "wt-person" }, el("span", { class: "wt-dot" }), p))),
      el("p", { class: "wt-note", text: g.itemId ? "Play, pause and skips happen for everyone. If someone's connection is slow, the party waits for them." : "Pick something to watch: everyone in the party starts it together." }),
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
    call("/SyncPlay/List", null, "GET").then((groups) => {
      const rows = (groups || []).map((x) => el("div", { class: "wt-group" },
        el("span", {}, el("b", { text: x.GroupName }), el("small", { text: x.Participants.join(", ") })),
        el("button", { class: "btn small primary", onclick: () => join(x.GroupId) }, "Join")));
      list.replaceChildren(...(rows.length ? rows : [el("p", { class: "wt-note", text: "No parties right now. Start one, or ask a friend to." })]));
    }).catch((e) => list.replaceChildren(el("p", { class: "wt-note", text: String(e) })));
  }
  dlg.replaceChildren(...parts);
}

/** How a friend gets in: their own profile, from Chillarr or Jellyfin's apps. */
function invite() {
  const m = store.media || {};
  const addr = m.tailscale || m.address;
  return el("details", { class: "wt-invite" },
    el("summary", { text: "How friends join" }),
    el("ol", {},
      el("li", { text: "Make them a profile here (the avatar at the top, then Add profile) and give it a password for Jellyfin apps." }),
      el("li", { text: addr ? `On their phone, TV or computer, open the Jellyfin app (or a browser) and connect to ${addr}.` : "On their phone, TV or computer, open the Jellyfin app and connect to this PC (Settings > Jellyfin & streaming shows the address)." }),
      el("li", { text: "Sign in with the profile's name and that password, open the menu, choose SyncPlay, and join your party." })));
}

export function openTogether() {
  if (!dlg.open) dlg.showModal();
  draw();
}

$("#together-btn").addEventListener("click", openTogether);
dlg.addEventListener("click", (e) => { if (e.target === dlg) dlg.close(); });
