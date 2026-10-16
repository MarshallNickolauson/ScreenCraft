import './style.css';

const nav = document.getElementById('nav');
const onScroll = () => nav?.classList.toggle('scrolled', scrollY > 8);
addEventListener('scroll', onScroll, { passive: true }); onScroll();

document.getElementById('burger')?.addEventListener('click', (e) => {
  const open = nav.classList.toggle('open');
  e.currentTarget.setAttribute('aria-expanded', String(open));
});

// highlight the current page in nav + docs sidebar
const path = location.pathname.replace(/\/index\.html$/, '/').replace(/\.html$/, '');
document.querySelectorAll('.nav-links a, .docs-nav a').forEach((a) => {
  const p = new URL(a.href, location.origin);
  if (p.hash === '' && p.pathname === path) a.classList.add('active');
});

const reduceMotion = matchMedia('(prefers-reduced-motion: reduce)').matches;
if (!reduceMotion) document.documentElement.classList.add('anim');

const store = {
  get(k) { try { return localStorage.getItem(k); } catch { return null; } },
  set(k, v) { try { localStorage.setItem(k, v); } catch { /* private mode: fine */ } },
};

// theme: dark unless the visitor chose otherwise (the choice is kept only in this browser)
const root = document.documentElement;
document.querySelectorAll('.theme-btn').forEach((b) => b.addEventListener('click', () => {
  const next = root.dataset.theme === 'light' ? 'dark' : 'light';
  root.dataset.theme = next;
  store.set('sc-theme', next);
}));

// the download buttons name the visitor's system
const isMac = !/Windows/i.test(navigator.userAgent);
document.querySelectorAll('[data-i18n-os]').forEach((el) => {
  el.textContent = isMac ? 'Download for macOS' : 'Download for Windows';
});
// split the headline into words so each can rise in turn (per character for CJK, which has no spaces)
function wrapWords(h) {
  if (!h || reduceMotion) return;
  const text = h.textContent;
  const seg = typeof Intl.Segmenter === 'function' ? new Intl.Segmenter(root.lang || 'en', { granularity: 'word' }) : null;
  const parts = seg ? [...seg.segment(text)].map((x) => x.segment) : text.split(/(\s+)/);
  h.setAttribute('aria-label', text);
  h.textContent = '';
  let i = 0;
  for (const part of parts) {
    if (!part.trim()) { h.append(' '); continue; }
    const w = document.createElement('span');
    w.className = 'w'; w.setAttribute('aria-hidden', 'true');
    const inner = document.createElement('span');
    inner.style.setProperty('--i', i++);
    inner.textContent = part;
    w.append(inner);
    h.append(w);
  }
}

// the key list's description may hold inline <kbd>, so group it into one grid cell
function wrapKeys() {
  document.querySelectorAll('.keys li').forEach((li) => {
    const first = li.firstElementChild;
    if (!first || li.querySelector(':scope > .d')) return;
    const d = document.createElement('span');
    d.className = 'd';
    while (first.nextSibling) d.append(first.nextSibling);
    li.append(d);
  });
}

wrapWords(document.querySelector('.hero h1'));
wrapKeys();

// the hero button downloads the installer for the visitor's system
document.querySelectorAll('[data-os-cta]').forEach((a) => {
  a.href = isMac ? a.dataset.mac : a.dataset.win;
});

// live GitHub star count, via our own cached endpoint
fetch('/api/stars').then((r) => r.json()).then(({ stars }) => {
  if (typeof stars !== 'number') return;
  const text = stars >= 1000 ? (stars / 1000).toFixed(stars >= 10000 ? 0 : 1).replace(/\.0$/, '') + 'k' : String(stars);
  document.querySelectorAll('.star-count').forEach((el) => { el.textContent = text; el.hidden = false; });
}).catch(() => {});

// scroll reveals: tag the pieces, stagger siblings, reveal as they enter
if (!reduceMotion && 'IntersectionObserver' in window) {
  const groups = ['.row-text > *', '.row > .frame, .row > .caption-demo', '.deflist > div', '.stance-text', '.closer .wrap > *', '.page-head > *', '.osrow > .os', '.note',
    '.sec > .wrap > h2', '.flow > *', '.compare-wrap', '.fine', '.keys li', '.whatsnew > *', '.more-link', '.faq details', '.oss > *', '.recbar', '.hero-video',
    '.prose > h1, .prose > h2, .prose > h3, .prose > p, .prose > ul, .prose > ol, .prose > table, .prose > pre, .prose > .grid2'];
  groups.forEach((sel) => {
    document.querySelectorAll(sel).forEach((el, i, all) => {
      el.setAttribute('data-reveal', '');
      const sibling = [...el.parentElement.children].indexOf(el);
      el.style.setProperty('--d', Math.min(sibling, 5) * 0.09 + 's');
    });
  });
  // Reveal by geometry rather than an observer: anything whose top has entered the viewport
  // (or is already above it) is shown. Cheap for ~20 elements and cannot miss an element.
  const pending = () => document.querySelectorAll('[data-reveal]:not(.in)');
  const check = () => {
    const edge = innerHeight * 0.94;
    pending().forEach((el) => { if (el.getBoundingClientRect().top < edge) el.classList.add('in'); });
  };
  let queued = false;
  const queue = () => { if (queued) return; queued = true; setTimeout(() => { queued = false; check(); }, 40); };
  addEventListener('scroll', queue, { passive: true });
  addEventListener('resize', queue);
  addEventListener('load', check);
  check();
  // and a safety net: whatever happens, nothing stays hidden
  setTimeout(() => document.querySelectorAll('[data-reveal]:not(.in)').forEach((el) => { if (el.getBoundingClientRect().top < innerHeight * 2) el.classList.add('in'); }), 4000);
}

// the demo video: click to pause, drag the bar to scrub
(() => {
  const v = document.getElementById('demoVideo');
  if (!v) return;
  const box = document.getElementById('heroVideo');
  const btn = box.querySelector('.vplay'), scrub = box.querySelector('.vscrub'), time = box.querySelector('.vtime');
  let dragging = false;
  const fmt = (s) => `${Math.floor(s / 60)}:${String(Math.floor(s % 60)).padStart(2, '0')}`;
  const sync = () => {
    btn.dataset.state = v.paused ? 'paused' : 'playing';
    btn.setAttribute('aria-label', v.paused ? 'Play' : 'Pause');
  };
  const tick = () => {
    if (!dragging && v.duration) scrub.value = String((v.currentTime / v.duration) * 1000);
    time.textContent = `${fmt(v.currentTime)} / ${fmt(v.duration || 0)}`;
    scrub.style.setProperty('--p', (Number(scrub.value) / 10) + '%');
  };
  const toggle = () => { if (v.paused) v.play().catch(() => {}); else v.pause(); };
  btn.addEventListener('click', toggle);
  v.addEventListener('click', toggle);
  const cls = () => box.classList.toggle('paused', v.paused);
  v.addEventListener('play', () => { sync(); cls(); }); v.addEventListener('pause', () => { sync(); cls(); });
  v.addEventListener('timeupdate', tick); v.addEventListener('loadedmetadata', tick);
  scrub.addEventListener('pointerdown', () => { dragging = true; });
  addEventListener('pointerup', () => { dragging = false; });
  scrub.addEventListener('input', () => {
    if (v.duration) v.currentTime = (Number(scrub.value) / 1000) * v.duration;
    tick();
  });
  sync(); tick();
})();

// the recorder bar, floated in three.js (falls back to the screenshot)
(() => {
  const c = document.querySelector('canvas[data-recbar]');
  if (!c) return;
  const wrap = c.parentElement;
  if (reduceMotion) { return; }
  const start = () => import('./recbar.js').then((m) => {
    wrap.classList.add('is-3d');            // the canvas must be displayed before it can be measured
    if (!m.mountRecBar(c)) wrap.classList.remove('is-3d');
  }).catch(() => wrap.classList.remove('is-3d'));
  if ('requestIdleCallback' in window) requestIdleCallback(start, { timeout: 1200 }); else setTimeout(start, 300);
})();

// the interactive editor demo (three.js), loaded when the section gets close
(() => {
  const root = document.getElementById('playground');
  if (!root) return;
  let started = false;
  const go = () => {
    if (started) return; started = true;
    import('./playground.js').then((m) => { if (!m.mountPlayground(root)) root.classList.remove('is-live'); }).catch(() => {});
  };
  const near = () => root.getBoundingClientRect().top < innerHeight * 2.2;
  const check = () => { if (near()) { go(); removeEventListener('scroll', check); removeEventListener('resize', check); } };
  addEventListener('scroll', check, { passive: true }); addEventListener('resize', check);
  setTimeout(check, 600);
})();

// three.js scenes load after first paint so they never delay the page
const canvases = document.querySelectorAll('canvas[data-scene]');
if (canvases.length && !reduceMotion) {
  const start = () => import('./scene.js').then((m) => canvases.forEach((c) => m.mountScene(c))).catch(() => {});
  if ('requestIdleCallback' in window) requestIdleCallback(start, { timeout: 1500 }); else setTimeout(start, 400);
}

// copy buttons (the one-line installer)
document.querySelectorAll('[data-copy]').forEach((b) => b.addEventListener('click', async () => {
  const el = document.querySelector(b.dataset.copy);
  if (!el) return;
  try { await navigator.clipboard.writeText(el.textContent.trim()); b.textContent = '✓'; } catch { return; }
  setTimeout(() => { b.textContent = b.dataset.label || 'Copy'; }, 1500);
}));
