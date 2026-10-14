// Things painted on top of the recording: annotations, the shortcut display and cut
// transitions. Shared by the preview and by export through render.js.

const mk = (w = 1, h = 1) => {
  if (typeof document !== "undefined") {
    const c = document.createElement("canvas");
    c.width = w;
    c.height = h;
    return c;
  }
  return new OffscreenCanvas(w, h);
};

const ease = (t) => t * t * (3 - 2 * t);
const clamp01 = (v) => Math.max(0, Math.min(1, v));

function rr(ctx, x, y, w, h, r) {
  ctx.beginPath();
  ctx.roundRect(x, y, w, h, Math.max(0, Math.min(r, Math.abs(w) / 2, Math.abs(h) / 2)));
}

/** Fade / pop factor 0..1 for an annotation at source time `t`. */
function animK(a, t) {
  if (a.anim === "none") return 1;
  const ms = Math.max(1, a.animMs || 250);
  return ease(clamp01(Math.min((t - a.sourceStartMs) / ms, (a.sourceEndMs - t) / ms)));
}

// letters with a right-to-left base direction (bidi classes R, AL)
const RTL = /[\p{Script=Arabic}\p{Script=Hebrew}\p{Script=Syriac}\p{Script=Thaana}\p{Script=Nko}\p{Script=Adlam}\p{Script=Hanifi_Rohingya}\p{Script=Mandaic}]/u;

/** True when the first strongly-directed letter is RTL — the same rule `dir=auto` uses. */
function startsRtl(text) {
  for (const ch of String(text || "")) {
    if (RTL.test(ch)) return true;
    if (/\p{L}/u.test(ch)) return false;
  }
  return false;
}

/** Words of a caption line in left-to-right draw order, keeping their logical indices. */
function wordOrder(ws, rtl) {
  // group same-direction words into runs; RTL runs draw right-to-left, and in an RTL
  // paragraph the run order itself is reversed
  const runs = [];
  ws.forEach((w, k) => {
    const r = RTL.test(w);
    const last = runs[runs.length - 1];
    if (last && last.rtl === r) last.w.push([w, k]);
    else runs.push({ rtl: r, w: [[w, k]] });
  });
  if (rtl) runs.reverse();
  return runs.flatMap((run) => (run.rtl ? run.w.reverse() : run.w));
}

const graphemes = (() => {
  const seg = typeof Intl !== "undefined" && Intl.Segmenter ? new Intl.Segmenter(undefined, { granularity: "grapheme" }) : null;
  return (s) => (seg ? [...seg.segment(s)].map((x) => x.segment) : [...s]);
})();

function wrapLines(ctx, text, maxW) {
  const out = [];
  for (const para of String(text || "").split("\n")) {
    let line = "";
    for (const word of para.split(/(\s+)/)) {
      const test = line + word;
      if (line && ctx.measureText(test).width > maxW) {
        out.push(line.trimEnd());
        line = word.trimStart();
      } else line = test;
      // a word wider than the box on its own (no spaces to break at, e.g. Lao) splits
      // between grapheme clusters so it still fits
      while (ctx.measureText(line).width > maxW) {
        const gs = graphemes(line);
        if (gs.length < 2) break;
        let n = gs.length - 1;
        while (n > 1 && ctx.measureText(gs.slice(0, n).join("")).width > maxW) n--;
        out.push(gs.slice(0, n).join(""));
        line = gs.slice(n).join("");
      }
    }
    out.push(line.trimEnd());
  }
  return out;
}

/**
 * Draw every annotation in `space` ("content" inside the clipped screen, "screen" over the
 * whole frame). `g` carries the geometry:
 *   { p, tSrc, W, H, s, zoomK, toX(nx), toY(ny), wPx(nw), hPx(nh), img, imgW, imgH, bounds }
 * `g.bounds` (a Map) receives each annotation's on-canvas box for editing handles.
 */
export function drawAnnotations(ctx, g, space) {
  const list = g.p.scene.annotations;
  if (!list || !list.length) return;
  for (const a of list) {
    if ((a.space || "screen") !== space) continue;
    if (g.tSrc < a.sourceStartMs || g.tSrc >= a.sourceEndMs) continue;
    const k = animK(a, g.tSrc);
    if (k <= 0.003) continue;
    const content = space === "content";
    const x = content ? g.toX(a.x) : a.x * g.W;
    const y = content ? g.toY(a.y) : a.y * g.H;
    let w = content ? g.wPx(a.w) : a.w * g.W;
    let h = content ? g.hPx(a.h) : a.h * g.H;
    const unit = g.s * (content ? g.zoomK : 1); // px per "1080p px"
    const lw = Math.max(1, (a.thickness || 5) * unit);
    const pop = a.anim === "pop" ? 0.85 + 0.15 * k : 1;
    ctx.save();
    ctx.globalAlpha *= a.anim === "pop" ? Math.min(1, k * 1.6) : k;

    if (a.kind === "text") {
      const fs = Math.max(6, (a.fontSize || 44) * unit * pop);
      ctx.font = `600 ${fs}px -apple-system, "SF Pro Display", "Segoe UI", Helvetica, Arial, sans-serif`;
      ctx.textBaseline = "top";
      const padX = fs * 0.5, padY = fs * 0.3;
      const maxW = Math.max(fs * 3, (a.w > 0 ? w : g.W * 0.8) - padX * 2);
      const lines = wrapLines(ctx, a.text, maxW);
      const tw = Math.max(...lines.map((l) => ctx.measureText(l).width), 1);
      const lh = fs * 1.22;
      const bw = tw + padX * 2, bh = lines.length * lh + padY * 2;
      w = bw;
      h = bh;
      const bx = x - (bw * (pop - 1)) / 2, by = y - (bh * (pop - 1)) / 2;
      if (a.fill && a.fill !== "none") {
        ctx.fillStyle = a.fill;
        rr(ctx, bx, by, bw, bh, fs * 0.28);
        ctx.fill();
      }
      ctx.fillStyle = a.color || "#fff";
      ctx.shadowColor = a.fill && a.fill !== "none" ? "transparent" : "rgba(0,0,0,.5)";
      ctx.shadowBlur = fs * 0.15;
      lines.forEach((l, i) => ctx.fillText(l, bx + padX, by + padY + i * lh));
      g.bounds?.set(a.id, { x: bx, y: by, w: bw, h: bh });
    } else if (a.kind === "box" || a.kind === "ellipse" || a.kind === "highlight") {
      const bx = x + (w - w * pop) / 2, by = y + (h - h * pop) / 2, bw = w * pop, bh = h * pop;
      ctx.strokeStyle = a.color || "#ffd60a";
      ctx.fillStyle = a.color || "#ffd60a";
      ctx.lineWidth = lw;
      ctx.lineJoin = "round";
      if (a.kind === "ellipse") {
        ctx.beginPath();
        ctx.ellipse(bx + bw / 2, by + bh / 2, Math.abs(bw) / 2, Math.abs(bh) / 2, 0, 0, Math.PI * 2);
        ctx.stroke();
      } else if (a.kind === "highlight") {
        ctx.globalAlpha *= 0.35;
        rr(ctx, bx, by, bw, bh, 6 * unit);
        ctx.fill();
      } else {
        rr(ctx, bx, by, bw, bh, 8 * unit);
        ctx.stroke();
      }
      g.bounds?.set(a.id, { x: bx, y: by, w: bw, h: bh });
    } else if (a.kind === "arrow") {
      const x2 = content ? g.toX(a.x2) : a.x2 * g.W;
      const y2 = content ? g.toY(a.y2) : a.y2 * g.H;
      const dx = x2 - x, dy = y2 - y;
      const len = Math.hypot(dx, dy) * pop || 1;
      const ux = dx / (Math.hypot(dx, dy) || 1), uy = dy / (Math.hypot(dx, dy) || 1);
      const ex = x + ux * len, ey = y + uy * len;
      const head = Math.min(len * 0.6, lw * 4 + 10 * unit);
      ctx.strokeStyle = a.color || "#ff453a";
      ctx.fillStyle = a.color || "#ff453a";
      ctx.lineWidth = lw;
      ctx.lineCap = "round";
      ctx.lineJoin = "round";
      ctx.beginPath();
      ctx.moveTo(x, y);
      ctx.lineTo(ex - ux * head * 0.6, ey - uy * head * 0.6);
      ctx.stroke();
      ctx.beginPath();
      ctx.moveTo(ex, ey);
      ctx.lineTo(ex - ux * head + -uy * head * 0.5, ey - uy * head + ux * head * 0.5);
      ctx.lineTo(ex - ux * head + uy * head * 0.5, ey - uy * head - ux * head * 0.5);
      ctx.closePath();
      ctx.fill();
      g.bounds?.set(a.id, { x: Math.min(x, x2), y: Math.min(y, y2), w: Math.abs(dx), h: Math.abs(dy), arrow: [x, y, x2, y2] });
    } else if (a.kind === "blur" || a.kind === "pixelate") {
      // re-draw the region of the recording through a tiny canvas: smooth = blur, nearest = pixels
      if (content && g.img && w > 1 && h > 1) {
        const str = clamp01(a.strength ?? 0.6);
        const cell = Math.max(2, (4 + str * 28) * g.s * 0.9);
        const sw = Math.max(2, Math.round(w / cell)), sh = Math.max(2, Math.round(h / cell));
        const c = (g.cache.blurC ??= mk(sw, sh));
        if (c.width !== sw || c.height !== sh) {
          c.width = sw;
          c.height = sh;
        }
        const cx = c.getContext("2d");
        cx.imageSmoothingEnabled = true;
        cx.imageSmoothingQuality = "high";
        const sxN = a.x * g.imgW, syN = a.y * g.imgH, swN = a.w * g.imgW, shN = a.h * g.imgH;
        const x0 = Math.max(0, sxN), y0 = Math.max(0, syN);
        const x1 = Math.min(g.imgW, sxN + swN), y1 = Math.min(g.imgH, syN + shN);
        if (x1 - x0 > 0.5 && y1 - y0 > 0.5) {
          cx.clearRect(0, 0, sw, sh);
          cx.drawImage(
            g.img, x0, y0, x1 - x0, y1 - y0,
            ((x0 - sxN) / swN) * sw, ((y0 - syN) / shN) * sh, ((x1 - x0) / swN) * sw, ((y1 - y0) / shN) * sh,
          );
          ctx.save();
          rr(ctx, x, y, w, h, 4 * unit);
          ctx.clip();
          ctx.imageSmoothingEnabled = a.kind === "blur";
          ctx.imageSmoothingQuality = "high";
          ctx.drawImage(c, x, y, w, h);
          ctx.restore();
        }
      }
      g.bounds?.set(a.id, { x, y, w, h });
    } else if (a.kind === "spotlight") {
      ctx.fillStyle = `rgba(0,0,0,${clamp01(a.strength ?? 0.65) * k})`;
      ctx.globalAlpha = 1;
      ctx.beginPath();
      ctx.rect(g.clipX ?? 0, g.clipY ?? 0, g.clipW ?? g.W, g.clipH ?? g.H);
      ctx.roundRect(x, y, w, h, 14 * unit);
      ctx.fill("evenodd");
      g.bounds?.set(a.id, { x, y, w, h });
    }
    ctx.restore();
  }
}

const MOD_SYMBOLS = [[8, "⌃", "Ctrl"], [4, "⌥", "Alt"], [2, "⇧", "Shift"], [1, "⌘", "Win"]];
const IS_MAC = typeof navigator !== "undefined" && /Mac/.test(navigator.platform || navigator.userAgent);

export function comboText(c) {
  const mods = MOD_SYMBOLS.filter(([bit]) => c.mods & bit);
  if (IS_MAC) return mods.map((m) => m[1]).join("") + (mods.length ? " " : "") + c.label;
  return [...mods.map((m) => (m[0] === 1 ? "Win" : m[2])), c.label].join(" + ");
}

/** Shortcut pill, bottom / top of the frame, fading in and out. */
export function drawKeys(ctx, W, H, cfg, combos, tOutMs, s) {
  if (!cfg?.enabled || !combos?.length) return;
  // latest combo at or before now (binary search over the sorted marks)
  let lo = 0, hi = combos.length;
  while (lo < hi) {
    const m = (lo + hi) >> 1;
    if (combos[m].tOutMs <= tOutMs) lo = m + 1;
    else hi = m;
  }
  const c = combos[lo - 1];
  if (!c) return;
  const hold = cfg.holdMs || 1400;
  const age = tOutMs - c.tOutMs;
  if (age < 0 || age > hold) return;
  const a = Math.min(1, age / 120, (hold - age) / 250);
  const fs = (cfg.size || 30) * s;
  const text = comboText(c);
  ctx.save();
  ctx.globalAlpha = ease(clamp01(a));
  ctx.font = `600 ${fs}px -apple-system, "SF Pro Display", "Segoe UI", Helvetica, Arial, sans-serif`;
  ctx.textBaseline = "middle";
  const tw = ctx.measureText(text).width;
  const padX = fs * 0.75, bh = fs * 1.9, bw = tw + padX * 2;
  const margin = fs * 1.4;
  const pos = cfg.position || "bottom-center";
  let x = (W - bw) / 2;
  if (pos.endsWith("left")) x = margin;
  else if (pos.endsWith("right")) x = W - bw - margin;
  const y = pos.startsWith("top") ? margin : H - bh - margin;
  const lift = (1 - ease(clamp01(age / 120))) * fs * 0.4;
  ctx.fillStyle = "rgba(18,20,28,.82)";
  ctx.shadowColor = "rgba(0,0,0,.35)";
  ctx.shadowBlur = fs * 0.5;
  rr(ctx, x, y + lift, bw, bh, bh / 2);
  ctx.fill();
  ctx.shadowColor = "transparent";
  ctx.strokeStyle = "rgba(255,255,255,.18)";
  ctx.lineWidth = Math.max(1, fs * 0.04);
  ctx.stroke();
  ctx.fillStyle = "#fff";
  ctx.fillText(text, x + padX, y + lift + bh / 2 + fs * 0.03);
  ctx.restore();
}

/** A logo / watermark image anchored in the frame (corners, edges or centre). */
export function drawWatermark(ctx, W, H, wm, img, s) {
  if (!wm || !wm.enabled || !wm.image || !img) return;
  const iw = img.naturalWidth || img.width, ih = img.naturalHeight || img.height;
  if (!iw || !ih) return;
  const w = Math.max(8, (wm.size || 0.12) * W);
  const h = (w * ih) / iw;
  const margin = 0.025 * Math.min(W, H) + 6 * s;
  const x = margin + (wm.x ?? 1) * (W - w - 2 * margin);
  const y = margin + (wm.y ?? 0) * (H - h - 2 * margin);
  ctx.save();
  ctx.globalAlpha *= clamp01(wm.opacity ?? 0.7);
  ctx.imageSmoothingQuality = "high";
  ctx.drawImage(img, x, y, w, h);
  ctx.restore();
}

/** Captions in the project's style (TikTok-like by default), centred in the bottom third. */
export function drawSubtitles(ctx, W, H, p, tSrc, s) {
  const list = p.scene.subtitles;
  if (!list || !list.length) return;
  const cap = list.find((c) => tSrc >= c.sourceStartMs && tSrc < c.sourceEndMs && (c.text || "").trim());
  if (!cap) return;
  const st = p.config.subtitles || {};
  const fs = (st.size || 56) * s;
  const text = st.uppercase ? cap.text.toUpperCase() : cap.text;
  ctx.save();
  ctx.font = `800 ${fs}px "Proxima Nova", -apple-system, "SF Pro Display", "Segoe UI", "Helvetica Neue", Arial, sans-serif`;
  ctx.textAlign = "center";
  ctx.textBaseline = "middle";
  ctx.lineJoin = "round";
  ctx.miterLimit = 2;
  const maxW = Math.max(fs * 4, W * (st.width || 0.34));
  const lines = wrapLines(ctx, text, maxW);
  const lh = fs * 1.16;
  const blockH = lines.length * lh;
  const cx = W / 2, cy = H * (st.y ?? 0.833);
  // Captions that follow each other swap instantly (no re-pop per line, which reads as
  // flicker). Only a line that starts after a pause eases in, and every line fades out softly.
  let k = 1;
  const prevEnd = list.reduce((m, c) => (c !== cap && c.sourceEndMs <= cap.sourceStartMs + 1 ? Math.max(m, c.sourceEndMs) : m), -1e9);
  const continuing = cap.sourceStartMs - prevEnd < 120;
  if (st.pop && !continuing) {
    const t = clamp01((tSrc - cap.sourceStartMs) / 140);
    k = 0.92 + 0.08 * (1 - Math.pow(1 - t, 3));
    ctx.globalAlpha = t;
  }
  const nextStart = list.reduce((m, c) => (c !== cap && c.sourceStartMs >= cap.sourceEndMs - 1 ? Math.min(m, c.sourceStartMs) : m), 1e9);
  const leaving = nextStart - cap.sourceEndMs >= 120;
  if (leaving) ctx.globalAlpha *= clamp01((cap.sourceEndMs - tSrc) / 110);
  ctx.translate(cx, cy);
  ctx.scale(k, k);
  if (st.background) {
    const tw = Math.max(...lines.map((l) => ctx.measureText(l).width));
    ctx.fillStyle = st.background;
    rr(ctx, -tw / 2 - fs * 0.45, -blockH / 2 - fs * 0.2, tw + fs * 0.9, blockH + fs * 0.4, fs * 0.3);
    ctx.fill();
  }
  const ow = (st.outlineWidth || 0) * s;
  // which word is being spoken: generated captions carry word start times, otherwise the
  // words share the caption's duration in proportion to their length
  const toks = text.split(/\s+/).filter(Boolean);
  let active = -1;
  if (st.highlight && toks.length) {
    const ws = cap.wordStarts;
    if (ws && ws.length === toks.length) {
      for (let i = 0; i < ws.length; i++) if (ws[i] <= tSrc) active = i;
    } else {
      const total = toks.reduce((n, w) => n + w.length + 1, 0);
      const f = clamp01((tSrc - cap.sourceStartMs) / Math.max(1, cap.sourceEndMs - cap.sourceStartMs));
      let acc = 0;
      active = toks.length - 1;
      for (let i = 0; i < toks.length; i++) {
        acc += (toks[i].length + 1) / total;
        if (f < acc) { active = i; break; }
      }
    }
  }
  // RTL captions are painted word by word, so the words have to be re-ordered for
  // drawing — one fillText call only shapes inside a single word
  const rtl = startsRtl(text);
  ctx.textAlign = "left";
  const space = ctx.measureText(" ").width;
  let wi = 0;
  lines.forEach((l, i) => {
    const y = -blockH / 2 + lh * (i + 0.5);
    const ws = l.split(/\s+/).filter(Boolean);
    const widths = ws.map((w) => ctx.measureText(w).width);
    const total = widths.reduce((a, b) => a + b, 0) + space * Math.max(0, ws.length - 1);
    const seq = wordOrder(ws, rtl);
    let x = -total / 2;
    const first = wi;
    if (ow > 0) {
      ctx.strokeStyle = st.outlineColor || "#000";
      ctx.lineWidth = ow * 2;
      seq.forEach(([w, k]) => { ctx.strokeText(w, x, y); x += widths[k] + space; });
    }
    x = -total / 2;
    seq.forEach(([w, k]) => {
      ctx.fillStyle = first + k === active ? st.highlight : st.color || "#fff";
      ctx.fillText(w, x, y);
      x += widths[k] + space;
    });
    wi += ws.length;
  });
  ctx.restore();
}

/**
 * Cut transition drawn over the finished frame. `kind` 1 fade, 2 flash, 3 blur, 4 slide,
 * 5 zoom; `p` is signed (+ before the cut, - after). Needs scratch space in `cache`.
 */
export function applyTransition(ctx, W, H, kind, p, cache, drawBackground) {
  const k = ease(clamp01(Math.abs(p)));
  if (!kind || kind === 6 || k <= 0.002) return; // 6 = cross-dissolve, blended in the compositor
  if (k <= 0.002) return;
  if (kind === 1) {
    ctx.save();
    ctx.globalAlpha = k;
    ctx.fillStyle = "#000";
    ctx.fillRect(0, 0, W, H);
    ctx.restore();
    return;
  }
  if (kind === 2) {
    ctx.save();
    ctx.globalAlpha = Math.pow(k, 1.4);
    ctx.fillStyle = "#fff";
    ctx.fillRect(0, 0, W, H);
    ctx.restore();
    return;
  }
  // the rest transform the picture itself: copy it aside first
  const t = (cache.transC ??= mk(W, H));
  if (t.width !== W || t.height !== H) {
    t.width = W;
    t.height = H;
  }
  const tc = t.getContext("2d");
  tc.clearRect(0, 0, W, H);
  tc.drawImage(ctx.canvas, 0, 0);
  if (kind === 3) {
    // blur dip: progressively smaller copies, scaled back up
    const steps = Math.max(1, Math.round(1 + k * 4));
    let cur = t;
    for (let i = 1; i <= steps; i++) {
      const c = mk(Math.max(2, Math.round(W / 2 ** i)), Math.max(2, Math.round(H / 2 ** i)));
      const g = c.getContext("2d");
      g.imageSmoothingQuality = "high";
      g.drawImage(cur, 0, 0, c.width, c.height);
      cur = c;
    }
    ctx.save();
    ctx.imageSmoothingQuality = "high";
    ctx.globalAlpha = Math.min(1, k * 1.4);
    ctx.drawImage(cur, 0, 0, W, H);
    ctx.restore();
    return;
  }
  ctx.save();
  ctx.clearRect(0, 0, W, H);
  drawBackground?.();
  if (kind === 4) {
    const dx = (p >= 0 ? -1 : 1) * k * W;
    ctx.drawImage(t, dx, 0);
  } else if (kind === 5) {
    const sc = 1 + k * 0.35;
    ctx.globalAlpha = 1 - k * 0.9;
    ctx.translate(W / 2, H / 2);
    ctx.scale(sc, sc);
    ctx.drawImage(t, -W / 2, -H / 2);
  }
  ctx.restore();
}
