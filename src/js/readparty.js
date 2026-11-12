// Read Party (readparty.rs): reading a series together, like Watch together
// is for films. Start one from the reader (the people button) and share the
// code; friends join from Read together in the top bar.
//
// Whoever started it is the host, and picks how the party reads:
//   Together: anyone who turns the page moves everyone, by page.
//   Follow me: only the host moves the party; everyone else's reader glides to
//     the host's exact spot (page plus how far down it, 0 to 1), sent as the
//     host scrolls and pushed to the others at once ("readparty-follow").
//   Free: everyone reads at their own pace; "Jump to the host" catches up.
// The host can also lock the party, ask for a password, and remove someone.
// The PC checks all of it; this page only shows it and asks.
import { ask, el, getPref, icon, invoke, listen, personKey, setPref, toast } from "./api.js";
import { openReader } from "./reader.js";

const PEOPLE = "M16 19v-1a4 4 0 0 0-4-4H7a4 4 0 0 0-4 4v1M12.5 7a3 3 0 1 1-6 0 3 3 0 0 1 6 0M21 19v-1a4 4 0 0 0-3-3.9M15.5 4.1a3 3 0 0 1 0 5.8";
const LOCK = "M7 11V7a5 5 0 0 1 10 0v4M5 11h14v10H5z";
// The host's scroll goes out at most this often in Follow me.
const SEND_MS = 200;
let party = null; // the last state from the PC (readparty.rs State)
let hooks = null; // the open reader's: where() and go(chapter, page, frac)
let pollT = 0;
let sendT = 0;
let sentAt = 0;
let quietUntil = 0;
let snapT = 0;
let catchT = 0;
// The host's spot this follower is heading for.
let target = null;

export const partyIcon = () => icon(PEOPLE);
export const inParty = () => !!party;
/** Follow me is on and this profile isn't the host: the host turns the pages. */
export const following = () => !!(party && party.mode === "follow" && !party.host);
/** May this reader's moves move the party? */
const leads = () => !!party && (party.mode === "together" || party.host);

/** The reader, while it's open: how to read where it is, and how to move it. */
export function attachReader(h) { hooks = h; if (party) poll(); paintNote(); }
export function detachReader() { hooks = null; paintNote(); }

/** A follower tried to turn the page or change chapter. */
export function nudge() { showNote("Following the host."); }

/** The reader turned a page: the party moves with it (when this one leads). */
export function moved() {
  if (!party || !hooks || Date.now() < quietUntil || !leads()) return;
  if (party.mode === "follow") { scrolled(); return; }
  clearTimeout(sendT);
  sendT = setTimeout(send, 350);
}

/** The reader scrolled. The host in Follow me sends its spot (at most every
 *  SEND_MS); a follower who scrolled away eases back to the host. */
export function scrolled() {
  if (!party || !hooks) return;
  if (following()) {
    clearTimeout(snapT);
    snapT = setTimeout(() => { if (following() && target) follow(target); }, 700);
    return;
  }
  if (party.mode !== "follow" || !party.host) return;
  if (sendT) return;
  sendT = setTimeout(send, Math.max(0, SEND_MS - (Date.now() - sentAt)));
}

async function send() {
  sendT = 0;
  sentAt = Date.now();
  const w = hooks && hooks.where();
  if (!w || !party || w.key !== party.key) return;
  try { take(await invoke("readparty_move", { code: party.code, chapter: w.chapter, page: w.page, frac: w.frac || 0 })); } catch (e) { ended(e); }
}

function take(s) {
  if (!party) return;
  const ver = Math.max(party.ver, s.ver);
  const was = party.mode;
  party = Object.assign({}, s, { ver });
  if (was !== s.mode && s.mode === "follow" && !s.host) {
    toast(`${s.host_name || "The host"} is leading. Following the host.`);
    follow(s);
  }
  paint();
  paintNote();
  if (dlg && dlg.open) panel();
}

/** Glides to the host's spot; a page not loaded here yet says "Catching up". */
async function follow(s) {
  target = { chapter: s.chapter, page: s.page, frac: s.frac };
  const w = hooks && hooks.where();
  if (!w || w.key !== party.key) return;
  quietUntil = Date.now() + 800;
  clearTimeout(catchT);
  const landed = await hooks.go(s.chapter, s.page, s.frac);
  if (landed === false) {
    showNote("Catching up");
    catchT = setTimeout(() => { if (following() && target) follow(target); }, 400);
  } else paintNote();
}

async function poll() {
  clearTimeout(pollT);
  if (!party) return;
  try {
    const s = await invoke("readparty_state", { code: party.code });
    if (!party) return;
    const newer = s.ver > party.ver;
    take(s);
    // Someone else moved: follow them (a page landing slightly off doesn't bounce back).
    const w = hooks && hooks.where();
    if (newer && !s.mine && w && w.key === s.key) {
      if (s.mode === "follow" && !s.host) follow(s);
      else if (s.mode === "together" && (w.chapter !== s.chapter || w.page !== s.page)) {
        quietUntil = Date.now() + 1500;
        hooks.go(s.chapter, s.page);
      }
    }
  } catch (e) { ended(e); return; }
  pollT = setTimeout(poll, document.hidden ? 4000 : 1000);
}

function ended(e) {
  if (!party) return;
  party = null;
  chatReset();
  target = null;
  clearTimeout(pollT);
  paint();
  paintNote();
  if (dlg && dlg.open) panel();
  toast(String(e).includes("ended") ? "The reading party ended." : String(e), true);
}

// Follow me: the host's spot, the moment it moves.
listen("readparty-follow", (e) => {
  const m = e.payload;
  if (!m || !party || m.code !== party.code || !following() || m.ver <= party.ver) return;
  party.ver = m.ver;
  follow(m);
});
// The host changed a setting, someone came or went, or this profile was removed.
listen("readparty-update", (e) => {
  const m = e.payload;
  if (m && party && m.code === party.code) poll();
});

async function start() {
  const w = hooks && hooks.where();
  if (!w) { toast("Open a chapter first, then start a reading party from the reader."); return; }
  try {
    const s = await invoke("readparty_start", { key: w.key, chapter: w.chapter, page: w.page });
    party = s;
    paint();
    poll();
    panel();
  } catch (e) { toast(String(e), true); }
}

async function join(code, password) {
  try {
    const s = await invoke("readparty_join", { code, password: password || null });
    party = s;
    close();
    // Open where the party is, then keep in step.
    await openReader(s.key, s.chapter);
    quietUntil = Date.now() + 1500;
    if (hooks) { if (following()) follow(s); else hooks.go(s.chapter, s.page); }
    paint();
    paintNote();
    poll();
    toast(`You're reading ${s.title} together.`);
    return "";
  } catch (e) { return String(e); }
}

async function leave() {
  if (!party) return;
  const code = party.code;
  party = null;
  target = null;
  clearTimeout(pollT);
  paint();
  paintNote();
  close();
  invoke("readparty_leave", { code }).catch(() => {});
  toast("You left the reading party.");
}

/* ---------- "Following the host" ---------- */
let note = null;
let noteText = null;
let noteT = 0;
/** Someone following can hide the note (its x, or the party panel); kept for next time. */
const noteOff = () => getPref("readparty-hide-note") === "1";
function setNoteOff(off) {
  setPref("readparty-hide-note", off ? "1" : "0");
  paintNote();
  if (off) toast("Hidden. Show it again from Read together.");
}
function makeNote() {
  noteText = el("span");
  const x = el("button", { class: "rp-note-x", "aria-label": "Hide this", title: "Hide this", onclick: () => setNoteOff(true) }, "×");
  note = el("div", { class: "rp-note", role: "status" }, noteText, x);
  document.body.append(note);
}
function showNote(text) {
  if (noteOff()) return;
  if (!note) makeNote();
  noteText.textContent = text;
  note.hidden = false;
  clearTimeout(noteT);
  noteT = setTimeout(paintNote, 2500);
}
/** The note stays up while following in the reader (unless hidden); otherwise it goes. */
function paintNote() {
  paintChat();
  const show = following() && hooks && !noteOff();
  if (!note) { if (!show) return; makeNote(); }
  noteText.textContent = "Following the host.";
  note.hidden = !show;
}

/* ---------- the panel ---------- */
let dlg = null;
function close() { if (dlg && dlg.open) dlg.close(); }
const MODES = [["together", "Together"], ["follow", "Follow me"], ["free", "Free"]];
const MODE_NOTE = {
  together: "Anyone who turns the page moves everyone.",
  follow: "Everyone follows the host's page and scroll.",
  free: "Everyone reads at their own pace.",
};

const set = (args) => invoke("readparty_set", Object.assign({ code: party.code }, args)).then(take).catch((e) => toast(String(e), true));
const hostCall = (cmd, who) => invoke(cmd, { code: party.code, who }).then(take).catch((e) => toast(String(e), true));

function segmented(label, items, on, pick) {
  return el("div", { class: "segmented", role: "group", "aria-label": label }, ...items.map(([v, text]) =>
    el("button", { class: on === v ? "on" : "", "aria-pressed": String(on === v), onclick: () => pick(v) }, text)));
}

/** The host's tools: reading mode, lock, people. Others see who leads and how. */
function hostTools() {
  if (!party.host) {
    const parts = [el("p", { class: "rp-people", text: `${party.host_name || "Someone"} is the host. ${MODE_NOTE[party.mode]}` })];
    if (party.mode === "free") parts.push(el("div", { class: "rp-acts rp-center" }, el("button", { class: "btn soft", onclick: () => { close(); if (hooks) hooks.go(party.chapter, party.page, party.frac); } }, "Jump to the host")));
    if (party.mode === "follow") {
      const sw = el("input", { type: "checkbox", class: "rp-note-sw", checked: !noteOff(), "aria-label": "Show \"Following the host\"" });
      sw.addEventListener("change", () => setNoteOff(!sw.checked));
      parts.push(el("label", { class: "rp-row" }, el("span", { text: "Show \"Following the host\"" }), sw));
    }
    return parts;
  }
  const parts = [
    el("div", { class: "rp-row" }, el("span", { text: "How we read" }), segmented("How we read", MODES, party.mode, (v) => set({ mode: v }))),
    el("p", { class: "rp-small", text: MODE_NOTE[party.mode] }),
  ];
  const pass = el("input", { class: "rp-pass", type: "password", maxlength: "32", placeholder: party.lock === "password" ? "New password (optional)" : "Password, 4 to 32 characters", "aria-label": "Party password", autocomplete: "new-password" });
  const save = el("button", { class: "btn soft", onclick: () => set({ lock: "password", password: pass.value }) }, "Save password");
  const passRow = el("div", { class: "rp-join", hidden: party.lock !== "password" }, pass, save);
  pass.addEventListener("keydown", (e) => { e.stopPropagation(); if (e.key === "Enter" || e.keyCode === 13) { e.preventDefault(); save.click(); } });
  parts.push(el("div", { class: "rp-row" }, el("span", { text: "Lock party" }), segmented("Lock party", [["open", "Open"], ["password", "Password"], ["locked", "Locked"]], party.lock, (v) => {
    if (v === "password") { passRow.hidden = false; pass.focus(); return; }
    set({ lock: v });
  })), passRow);
  const others = (party.who || []).filter((p) => !p.host);
  if (others.length) {
    parts.push(el("div", { class: "rp-list" }, ...others.map((p) => el("div", { class: "rp-person" },
      el("b", { text: p.name }),
      el("span", { class: "rp-acts" },
        el("button", { class: "btn small ghost", onclick: () => hostCall("readparty_host", p.id) }, "Make host"),
        el("button", { class: "btn small ghost", onclick: async () => {
          if (await ask(`Remove ${p.name} from the party?`, "They can't come back for 10 minutes unless you let them.", [], "Remove", "Cancel")) hostCall("readparty_kick", p.id);
        } }, "Kick"))))));
  }
  if (party.removed && party.removed.length) {
    parts.push(el("p", { class: "rp-small", text: "Removed" }), el("div", { class: "rp-list" }, ...party.removed.map((p) => el("div", { class: "rp-person" },
      el("span", {}, el("b", { text: p.name }), el("small", { text: ` Can come back in ${p.mins} min` })),
      el("button", { class: "btn small ghost", onclick: () => hostCall("readparty_allow", p.id) }, "Allow back")))));
  }
  return parts;
}

/** Read together: the code to share and Leave, or a box to join a friend's. */
export function panel() {
  if (!dlg) {
    dlg = el("dialog", { class: "ask rp-dialog", "aria-label": "Read together" });
    dlg.addEventListener("click", (e) => { if (e.target === dlg) close(); });
    document.body.append(dlg);
  }
  const head = el("div", { class: "rp-head" }, el("span", { class: "rp-icon" }, icon(PEOPLE)), el("h2", { text: "Read together" }));
  if (party) {
    const copy = el("button", { class: "btn soft", onclick: async () => {
      try { await navigator.clipboard.writeText(party.code); copy.textContent = "Copied"; } catch (e) { toast(`The code is ${party.code}`); }
    } }, "Copy code");
    const code = el("div", { class: "rp-code", text: party.code });
    if (party.lock !== "open") code.prepend(el("span", { class: "rp-lock", title: party.lock === "locked" ? "Locked" : "Needs a password" }, icon(LOCK)));
    dlg.replaceChildren(head,
      el("p", { class: "sub", text: `Reading ${party.title}. Share this code to read together.` }),
      code,
      el("p", { class: "rp-people", text: party.people === 1 ? "Just you so far" : `${party.people} reading together` }),
      ...hostTools(),
      el("div", { class: "rp-acts" }, copy, el("button", { class: "btn ghost", onclick: leave }, "Leave"), el("button", { class: "btn primary", onclick: close }, "Done")));
  } else {
    const input = el("input", { class: "rp-input", type: "text", maxlength: "8", placeholder: "Code", "aria-label": "Party code", autocomplete: "off", spellcheck: "false" });
    const pass = el("input", { class: "rp-pass", type: "password", maxlength: "32", placeholder: "Password", "aria-label": "Party password", autocomplete: "off", hidden: true });
    const err = el("p", { class: "rp-small rp-err", hidden: true });
    const go = async () => {
      if (!input.value.trim()) return;
      const e = await join(input.value, pass.value);
      if (!e) return;
      // A password party: the box comes up for it.
      if (/password/i.test(e)) { pass.hidden = false; pass.focus(); pass.select(); }
      err.textContent = e;
      err.hidden = false;
    };
    const goBtn = el("button", { class: "btn primary", onclick: go }, "Join");
    for (const i of [input, pass]) i.addEventListener("keydown", (e) => { e.stopPropagation(); if ((e.key === "Enter" || e.keyCode === 13) && input.value.trim()) { e.preventDefault(); go(); } });
    const startBtn = hooks ? el("button", { class: "btn soft", onclick: start }, "Start one here") : null;
    dlg.replaceChildren(head,
      el("p", { class: "sub", text: hooks ? "Start a party on this chapter and share the code, or join a friend's." : "Got a code from a friend? Join their party. To start one, open a chapter and press the people button in the reader." }),
      el("div", { class: "rp-join" }, input, goBtn),
      el("div", { class: "rp-join" }, pass),
      err,
      el("div", { class: "rp-acts" }, startBtn, el("button", { class: "btn ghost", onclick: close }, "Close")));
    requestAnimationFrame(() => input.focus());
  }
  if (!dlg.open) dlg.showModal();
}

/** The buttons show when you're in a party, and how many are reading. */
function paint() {
  for (const b of document.querySelectorAll(".rp-btn")) {
    b.classList.toggle("on", !!party);
    b.title = party ? `Reading together (${party.people})` : "Read together";
    let n = b.querySelector(".rp-n");
    if (party && party.people > 1) {
      if (!n) { n = el("span", { class: "rp-n" }); b.append(n); }
      n.textContent = String(party.people);
    } else if (n) n.remove();
  }
}

// The top bar's Read together, next to Watch together (shown while reading).
(function topButton() {
  const watch = document.getElementById("together-btn");
  if (!watch) return;
  const b = el("button", { class: "icon-btn rp-btn rp-top", id: "read-together", "aria-label": "Read together", title: "Read together", onclick: () => panel() }, icon(PEOPLE));
  watch.after(b);
})();

/* ---------- Chat (#76) ----------
   Like Watch together's: names, 300 characters, the same rate limit, and a
   switch for messages popping up while you read. On a wide screen it sits on
   the right and the page gets narrower, so it never covers what you read; on
   a phone it's a small sheet at the bottom that shrinks to the chat button
   with one tap, and never blocks scrolling or turning pages. */
const CHAT_MAX = 300;
const FLOAT_MS = 6000;
const QUIET_PREF = "chillarr-readchat-float";
const chatQuiet = () => getPref(QUIET_PREF) === "0";
const said = [];
let unread = 0;
let chatOpen = false;
const CHAT = "M4 5h16v11H9l-5 4z";
const rpBadge = el("span", { class: "chat-badge", hidden: true });
const rpBtn = el("button", { class: "rp-chat-btn", "aria-label": "Party chat", title: "Party chat", hidden: true, onclick: () => toggleReadChat() }, icon(CHAT), rpBadge);
const rpList = el("ol", { class: "chat-list", "aria-live": "polite" });
const rpInput = el("input", { type: "text", class: "chat-input", maxlength: String(CHAT_MAX), placeholder: "Say something", "aria-label": "Message to the reading party", autocomplete: "off" });
const rpSend = el("button", { class: "btn small primary", type: "submit" }, "Send");
const rpForm = el("form", { class: "chat-form" }, rpInput, rpSend);
const rpQuietSw = el("input", { type: "checkbox", class: "switch", "aria-label": "Show messages while reading" });
const rpPanel = el("aside", { class: "rp-chat", role: "dialog", "aria-label": "Party chat", hidden: true },
  el("div", { class: "chat-head" }, el("b", { text: "Party chat" }), el("button", { class: "link-btn", type: "button", onclick: () => toggleReadChat(false) }, "Hide")),
  rpList, rpForm,
  el("label", { class: "chat-quiet" }, rpQuietSw, el("span", { text: "Show messages while reading" })));
const rpFloat = el("ol", { class: "chat-float rp-float", "aria-hidden": "true" });
document.body.append(rpBtn, rpPanel, rpFloat);

rpQuietSw.addEventListener("change", () => { setPref(QUIET_PREF, rpQuietSw.checked ? "1" : "0"); if (!rpQuietSw.checked) rpFloat.replaceChildren(); });
// Typing is for the message, not the reader's keys (arrows turn pages).
for (const n of [rpInput, rpQuietSw]) n.addEventListener("keydown", (e) => { e.stopPropagation(); if (e.key === "Escape") { e.preventDefault(); toggleReadChat(false); } });
rpPanel.addEventListener("click", (e) => e.stopPropagation());

const timeOf = (at) => new Date(at).toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
const chatLine = (m) => el("li", { class: m.from_id === personKey() ? "chat-line mine" : "chat-line" },
  el("b", { text: m.from }), el("span", { text: m.text }), el("time", { text: timeOf(m.at) }));

function drawRpBadge() {
  rpBadge.hidden = !unread;
  rpBadge.textContent = unread > 9 ? "9+" : String(unread);
}

/** The chat shows only in a party, with the reader open. */
function paintChat() {
  const can = !!(party && hooks);
  rpBtn.hidden = !can || chatOpen;
  rpPanel.hidden = !can || !chatOpen;
  rpFloat.hidden = !can;
  document.body.classList.toggle("rp-chat-open", can && chatOpen);
}

/** Opens or shuts the chat (no argument: the other way). */
export function toggleReadChat(on = !chatOpen) {
  chatOpen = !!on && !!party;
  if (chatOpen) {
    rpQuietSw.checked = !chatQuiet();
    unread = 0;
    drawRpBadge();
    rpFloat.replaceChildren();
  } else if (rpPanel.contains(document.activeElement)) document.activeElement.blur();
  paintChat();
  if (chatOpen) { rpList.scrollTop = rpList.scrollHeight; if (matchMedia("(min-width: 761px)").matches) rpInput.focus(); }
}

function chatReset() {
  said.length = 0;
  unread = 0;
  rpList.replaceChildren();
  rpFloat.replaceChildren();
  chatOpen = false;
  drawRpBadge();
  paintChat();
}

listen("readparty-chat", (e) => {
  const m = e.payload;
  if (!m || !party || m.code !== party.code || said.some((x) => x.id === m.id)) return;
  said.push(m);
  if (said.length > 200) said.shift();
  if (rpList.children.length > 400) rpList.firstChild.remove();
  const near = rpList.scrollHeight - rpList.scrollTop - rpList.clientHeight < 40;
  rpList.append(chatLine(m));
  const mine = m.from_id === personKey();
  if (near || mine) rpList.scrollTop = rpList.scrollHeight;
  if (chatOpen || mine) return;
  unread++;
  drawRpBadge();
  if (chatQuiet() || !hooks) return;
  // While it's shut: the message floats up beside the page, then fades.
  const f = chatLine(m);
  rpFloat.append(f);
  while (rpFloat.children.length > 3) rpFloat.firstChild.remove();
  setTimeout(() => { f.classList.add("out"); setTimeout(() => f.remove(), 600); }, FLOAT_MS);
});

rpForm.addEventListener("submit", async (e) => {
  e.preventDefault();
  const text = rpInput.value.trim();
  if (!text || !party) return;
  rpSend.disabled = true;
  try {
    await invoke("readparty_say", { code: party.code, text });
    rpInput.value = "";
  } catch (err) { toast(String(err), true); }
  rpSend.disabled = false;
  rpInput.focus();
});
