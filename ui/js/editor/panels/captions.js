import { S, P, hooks, uid, fmt } from "../state.js";
import { invoke, dialog, baseName, listen } from "../../tauri.js";
import { dir } from "../state.js";
import { change } from "../history.js";
import { h3, el, slider, toggle, segmented, colorField } from "../widgets.js";
import { addCaptionAtPlayhead, deleteSelected } from "../timeline.js";
import { outToSrc, srcToOut } from "../timemap.js";
import { parseSubtitles, writeSrt } from "../srt.js";

/** Style presets; position and width are left as the user set them, except TikTok's own. */
export const SUBTITLE_PRESETS = {
  tiktok: { style: "tiktok", size: 70, color: "#ffffff", outlineColor: "#000000", outlineWidth: 7, background: "", uppercase: true, pop: true, highlight: "#ffe14a", y: 0.8, width: 0.6 },
  classic: { style: "classic", size: 46, color: "#ffffff", outlineColor: "#000000", outlineWidth: 0, background: "#000000b3", uppercase: false, pop: false, highlight: "" },
  minimal: { style: "minimal", size: 42, color: "#ffffff", outlineColor: "#000000", outlineWidth: 3, background: "", uppercase: false, pop: false, highlight: "" },
};
const PRESET_NAMES = [["tiktok", "TikTok"], ["classic", "Classic"], ["minimal", "Minimal"]];

async function importSrt() {
  const picked = await dialog.open({ multiple: false, filters: [{ name: "Subtitles", extensions: ["srt", "vtt"] }] });
  const path = Array.isArray(picked) ? picked[0] : picked;
  if (!path) return;
  let items;
  try {
    items = parseSubtitles(await invoke("read_text_file", { path }));
  } catch (e) {
    alert("Could not read that file: " + (e?.message || e));
    return;
  }
  if (!items.length) {
    alert("No captions found in that file.");
    return;
  }
  // SRT times describe the finished video, so map them back onto the recording
  change(null, () => {
    P().scene.subtitles = P().scene.subtitles || [];
    for (const it of items) {
      P().scene.subtitles.push({
        id: uid(), text: it.text,
        sourceStartMs: outToSrc(it.start).src,
        sourceEndMs: Math.max(outToSrc(it.start).src + 200, outToSrc(it.end).src),
      });
    }
    S.selected = null;
  }, { panel: true, timeline: true, track: false });
}

async function exportSrt() {
  const items = (P().scene.subtitles || [])
    .map((c) => ({ start: srcToOut(c.sourceStartMs), end: srcToOut(c.sourceEndMs), text: c.text }))
    .filter((c) => c.start != null && c.end != null && (c.text || "").trim())
    .sort((a, b) => a.start - b.start);
  if (!items.length) {
    alert("There are no captions to export.");
    return;
  }
  const base = (P().name || "captions").replace(/[\\/:]/g, "-");
  const path = await dialog.save({ defaultPath: `${base}.srt`, filters: [{ name: "SRT", extensions: ["srt"] }] });
  if (!path) return;
  const bytes = new TextEncoder().encode(writeSrt(items));
  await invoke("export_begin", { path });
  await invoke("export_write", bytes, { headers: { "x-position": "0" } });
  await invoke("export_end");
}

const WHISPER_MODELS = [
  ["small.en", "Accurate · English · 488 MB"],
  ["large-v3-turbo-q5_0", "Most accurate · multilingual · 574 MB"],
  ["base.en", "Balanced · English · 148 MB"],
  ["tiny.en", "Fastest · English · 78 MB"],
  ["base", "Multilingual · 148 MB"],
];
const WHISPER_LANGS = [
  ["auto", "Auto"],
  ["en", "English"],
  ["es", "Spanish"],
  ["fr", "French"],
  ["de", "German"],
  ["ar", "Arabic"],
  ["it", "Italian"],
  ["lo", "Lao"],
];
const STAGE = { audio: "Reading audio…", model: "Loading model…", transcribe: "Transcribing", download: "Downloading model", done: "Done" };
const whisper = { model: "small.en", language: "auto", busy: false, msg: "", have: {} };

async function generateCaptions(rerender) {
  if (whisper.busy) return;
  whisper.busy = true;
  whisper.msg = "Starting…";
  rerender();
  const off = await listen("whisper-progress", (e) => {
    const { stage, pct } = e.payload;
    whisper.msg = `${STAGE[stage] || stage}${stage === "transcribe" || stage === "download" ? " " + Math.round(pct) + "%" : ""}`;
    const m = document.getElementById("whisper-status");
    if (m) m.textContent = whisper.msg;
  });
  try {
    const caps = await invoke("whisper_transcribe", { dir, project: P(), model: whisper.model, language: whisper.language });
    if (!caps.length) {
      alert("No speech was found in this recording.");
    } else {
      change(null, () => {
        P().scene.subtitles = P().scene.subtitles || [];
        // first captions in a project: start from the TikTok look
        if (!P().scene.subtitles.length) Object.assign(P().config.subtitles, SUBTITLE_PRESETS.tiktok);
        for (const c of caps) P().scene.subtitles.push({ id: uid(), text: c.text, sourceStartMs: c.startMs, sourceEndMs: c.endMs, wordStarts: c.wordStarts || [] });
        S.selected = null;
      }, { panel: true, timeline: true, track: false });
    }
  } catch (e) {
    alert("Caption generation failed: " + (e?.message || e));
  } finally {
    off?.();
    whisper.busy = false;
    whisper.msg = "";
    whisper.have = {};
    rerender();
  }
}

export function panelCaptions(p) {
  const st = P().config.subtitles;
  // presets that no longer exist (TikTok red, Highlight) fall back to TikTok
  if (!SUBTITLE_PRESETS[st.style]) Object.assign(st, SUBTITLE_PRESETS.tiktok);
  const list = (P().scene.subtitles || []).slice().sort((a, b) => a.sourceStartMs - b.sourceStartMs);
  p.append(h3("Captions"));
  p.append(el('<div class="muted">Type captions or import an SRT file. They sit in the middle of the bottom third of the video, like TikTok.</div>'));
  // on-device speech recognition (whisper.cpp); runs locally, nothing is uploaded
  p.append(h3("Auto captions"));
  p.append(el('<div class="muted">Transcribes the microphone on this computer with whisper.cpp. The first run downloads the model you pick (once); the audio never leaves your machine.</div>'));
  // long labels do not fit a segmented control, so the model is a drop-down
  const pick = el('<div class="field"><label><span>Model</span></label><select></select></div>');
  const sel = pick.querySelector("select");
  for (const [id, label] of WHISPER_MODELS) sel.add(new Option(label, id, false, id === whisper.model));
  sel.onchange = () => {
    whisper.model = sel.value;
    // an English-only model cannot serve the chosen language
    if (sel.value.endsWith(".en") && whisper.language !== "auto" && whisper.language !== "en") whisper.language = "en";
    hooks.renderPanel();
  };
  sel.onkeydown = (e) => e.stopPropagation();
  p.append(pick);
  // eight options do not fit a segmented row, so the language is a drop-down like the model
  const lpick = el('<div class="field"><label><span>Language</span></label><select></select></div>');
  const lsel = lpick.querySelector("select");
  for (const [id, label] of WHISPER_LANGS) lsel.add(new Option(label, id, false, id === whisper.language));
  lsel.onchange = () => {
    whisper.language = lsel.value;
    // non-English speech needs a multilingual model; swap an English-only one for `base`
    if (lsel.value !== "auto" && lsel.value !== "en" && whisper.model.endsWith(".en")) whisper.model = "base";
    hooks.renderPanel();
  };
  lsel.onkeydown = (e) => e.stopPropagation();
  p.append(lpick);
  const gen = el(`<button class="btn-line" ${whisper.busy ? "disabled" : ""}>${whisper.busy ? "Working…" : "Generate captions"}</button>`);
  gen.onclick = () => generateCaptions(hooks.renderPanel);
  p.append(gen);
  if (whisper.busy) p.append(el(`<div class="hint muted" id="whisper-status">${whisper.msg}</div>`));
  p.append(el('<div class="hint muted">Speech recognition by whisper.cpp (github.com/ggml-org/whisper.cpp, MIT) with OpenAI Whisper models (MIT).</div>'));
  const row = el('<div class="btn-row"></div>');
  const add = el('<button class="btn-line">Add caption</button>');
  add.onclick = addCaptionAtPlayhead;
  const imp = el('<button class="btn-line">Import SRT…</button>');
  imp.onclick = importSrt;
  const exp = el('<button class="btn-line">Export SRT…</button>');
  exp.onclick = exportSrt;
  row.append(add, imp, exp);
  p.append(row);

  const cap = S.selected?.type === "caption" && list.find((c) => c.id === S.selected.id);
  if (cap && S.multi?.length > 1) {
    p.append(h3(`${S.multi.length} captions selected`));
    p.append(el('<div class="hint muted">Drag any selected caption to move them all together. ⌘ / Ctrl-click adds or removes one, Shift-click selects a range, ⌘ / Ctrl + A selects all.</div>'));
    const del = el(`<button class="btn-line danger">Delete ${S.multi.length} captions</button>`);
    del.onclick = deleteSelected;
    p.append(del);
    const clr = el('<button class="btn-line">Deselect</button>');
    clr.onclick = () => { S.selected = null; S.multi = null; hooks.renderTimeline(); hooks.renderPanel(); };
    p.append(clr);
  } else if (cap) {
    p.append(h3("Selected caption"));
    const w = el('<div class="field"><label><span>Text</span></label><textarea rows="3" spellcheck="false"></textarea></div>');
    const ta = w.querySelector("textarea");
    ta.dir = "auto";
    ta.value = cap.text || "";
    ta.oninput = () => change("cap-" + cap.id, () => (cap.text = ta.value), { track: false, timeline: true });
    ta.onkeydown = (e) => e.stopPropagation();
    p.append(w);
    const len = ((cap.sourceEndMs - cap.sourceStartMs) / 1000).toFixed(1);
    p.append(el(`<div class="hint muted">${len} s. Drag the block on the Captions track to move it, its ends to trim.</div>`));
    const del = el('<button class="btn-line danger">Delete caption</button>');
    del.onclick = deleteSelected;
    p.append(del);
  } else if (list.length) {
    p.append(h3(`${list.length} caption${list.length === 1 ? "" : "s"}`));
    for (const c of list.slice(0, 60)) {
      const r = el('<button class="note-row" dir="auto"></button>');
      const t0 = srcToOut(c.sourceStartMs);
      r.textContent = `${t0 != null ? fmt(t0) : "—"}  ${(c.text || "").replace(/\s+/g, " ").slice(0, 34) || "(empty)"}`;
      r.onclick = () => {
        S.selected = { type: "caption", id: c.id };
        hooks.seek(t0 ?? 0);
        hooks.renderTimeline();
        hooks.renderPanel();
      };
      p.append(r);
    }
  }

  p.append(h3("Style"));
  p.append(segmented("Preset", PRESET_NAMES, () => st.style, (v) => Object.assign(st, SUBTITLE_PRESETS[v]), { track: false }));
  const opt = { track: false };
  p.append(slider("Size", 24, 120, 1, () => st.size, (v) => (st.size = v), { fmt: (v) => v + " px", ...opt, key: "sub-size" }));
  p.append(slider("Height", 0.15, 0.95, 0.01, () => st.y, (v) => (st.y = v), { fmt: (v) => Math.round(v * 100) + "%", ...opt, key: "sub-y" }));
  p.append(slider("Width", 0.2, 0.95, 0.01, () => st.width, (v) => (st.width = v), { fmt: (v) => Math.round(v * 100) + "%", ...opt, key: "sub-w" }));
  p.append(colorField("Text color", () => st.color, (v) => (st.color = v)));
  p.append(toggle("Highlight spoken word", "Colours each word as it is said.", () => !!st.highlight, (v) => (st.highlight = v ? "#ffe14a" : ""), { track: false, panel: true }));
  if (st.highlight) p.append(colorField("Highlight color", () => st.highlight, (v) => (st.highlight = v)));
  p.append(slider("Stroke width", 0, 30, 1, () => st.outlineWidth, (v) => (st.outlineWidth = v), { fmt: (v) => v + " px", ...opt, key: "sub-ow" }));
  if (st.outlineWidth > 0) p.append(colorField("Stroke color", () => st.outlineColor, (v) => (st.outlineColor = v)));
  p.append(toggle("Capital letters", null, () => st.uppercase, (v) => (st.uppercase = v), opt));
  p.append(toggle("Pop in", "A quick bounce when each caption appears.", () => st.pop, (v) => (st.pop = v), opt));
  const hasBox = !!st.background;
  p.append(toggle("Box behind text", null, () => hasBox, (v) => (st.background = v ? "#000000b3" : ""), opt));
  void baseName;
}
