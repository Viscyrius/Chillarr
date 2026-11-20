// Which audio track to start in. Pure, so it can be tested on its own
// (tests/js/audiopick.test.mjs); player.js asks it on every new title.

/** Languages offered for dubs, with how a file's audio track names them. */
export const AUDIO_LANGS = [
  ["eng", "English", /^en/i], ["spa", "Spanish", /^(es|spa)/i], ["fre", "French", /^(fr)/i], ["ger", "German", /^(de|ger)/i],
  ["ita", "Italian", /^it/i], ["por", "Portuguese", /^(pt|por)/i], ["rus", "Russian", /^ru/i], ["hin", "Hindi", /^hi/i],
  ["ara", "Arabic", /^ar/i], ["tur", "Turkish", /^tr|^tur/i], ["jpn", "Japanese", /^(ja|jp)/i], ["kor", "Korean", /^ko/i], ["zho", "Chinese", /^(zh|ch[is])/i],
];

export const isEnglish = (lang) => !lang || /^en/i.test(lang);

/** True when a track is in `code`: by its language tag, or by its name when it has no tag. */
export function speaks(track, code) {
  const row = AUDIO_LANGS.find(([c]) => c === code);
  if (!row) return (track.lang || "") === code;
  if (track.lang) return row[2].test(track.lang);
  return new RegExp(`(^|[^a-z])${row[1]}([^a-z]|$)`, "i").test(track.label || "");
}

/**
 * `tracks`: the file's audio ({index, lang, label, default}). `settings`:
 * {audio_pref, audio_lang}. `handLang`: the language picked by hand for this
 * show, or null. Returns the track to switch to (null: keep the file's own
 * start) and whether the dub asked for isn't in the file at all.
 */
export function chooseAudio(tracks, settings, handLang) {
  const def = tracks.find((a) => a.default) || tracks[0];
  if (!def) return { index: null, missing: false };
  // A language picked by hand wins, also when the file already starts in it.
  if (handLang) {
    if (speaks(def, handLang)) return { index: null, missing: false };
    const t = tracks.find((a) => speaks(a, handLang));
    if (t) return { index: t.index, missing: false };
  }
  if (settings?.audio_pref === "dub") {
    const want = settings.audio_lang || "eng";
    if (speaks(def, want)) return { index: null, missing: false };
    const t = tracks.find((a) => speaks(a, want));
    return t ? { index: t.index, missing: false } : { index: null, missing: true };
  }
  // Original voices: only languages films are rarely dubbed into, so an English
  // film with an extra Hindi or French track stays in English.
  if (!isEnglish(def.lang)) return { index: null, missing: false };
  const t = tracks.find((a) => /^(ja|jp|ko|zh|ch[is])/i.test(a.lang || ""));
  return { index: t ? t.index : null, missing: false };
}
