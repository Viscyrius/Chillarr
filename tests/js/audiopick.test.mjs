// Run: node --test tests/js
import test from "node:test";
import assert from "node:assert/strict";
import { chooseAudio } from "../../src/js/audiopick.js";

const jpnEng = [{ index: 1, lang: "jpn", label: "Japanese - AAC", default: true }, { index: 2, lang: "eng", label: "English - AAC", default: false }];
const dub = { audio_pref: "dub", audio_lang: "eng" };
const sub = { audio_pref: "sub" };

test("dub picks the English track over a Japanese default", () => {
  assert.deepEqual(chooseAudio(jpnEng, dub, null), { index: 2, missing: false });
});

test("an English track with no language tag is found by its name", () => {
  const a = [{ index: 1, lang: "jpn", label: "Japanese", default: true }, { index: 3, lang: null, label: "English 5.1", default: false }];
  assert.deepEqual(chooseAudio(a, dub, null), { index: 3, missing: false });
});

test("dub with no English track says it's missing", () => {
  const a = [{ index: 1, lang: "jpn", label: "Japanese", default: true }, { index: 2, lang: "por", label: "Portuguese", default: false }];
  assert.deepEqual(chooseAudio(a, dub, null), { index: null, missing: true });
});

test("a language picked by hand for this show beats the dub setting, even when the file starts in it", () => {
  assert.deepEqual(chooseAudio(jpnEng, dub, "jpn"), { index: null, missing: false });
});

test("a hand pick in a language the file lacks falls back to the setting", () => {
  assert.deepEqual(chooseAudio(jpnEng, dub, "fre"), { index: 2, missing: false });
});

test("original voices: an English default with a Japanese track switches to Japanese", () => {
  const a = [{ index: 1, lang: "eng", label: "English", default: true }, { index: 2, lang: "jpn", label: "Japanese", default: false }];
  assert.deepEqual(chooseAudio(a, sub, null), { index: 2, missing: false });
});

test("original voices: an English film with a Hindi track stays English", () => {
  const a = [{ index: 1, lang: "eng", label: "English", default: true }, { index: 2, lang: "hin", label: "Hindi", default: false }];
  assert.deepEqual(chooseAudio(a, sub, null), { index: null, missing: false });
});

test("one track: nothing to choose, nothing missing in sub mode", () => {
  assert.deepEqual(chooseAudio([jpnEng[0]], sub, null), { index: null, missing: false });
});
