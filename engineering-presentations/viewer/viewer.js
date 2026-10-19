/* Keep the old revision visible until the new document and its images load. */
'use strict';
const $ = id => document.getElementById(id);
let loaded = null;
let latest = null;
let frame = null;
let busy = false;
let request = 0;

function position() {
  const query = new URLSearchParams(location.hash.slice(1));
  const page = Math.max(1, Number.parseInt(frame?.contentWindow.location.hash.slice(1) || query.get('page'), 10) || 1);
  return { page, id: loaded ? loaded.slides[page - 1] : query.get('slide') };
}
function remember() {
  const at = position();
  const query = new URLSearchParams({ page: at.page });
  if (at.id) query.set('slide', at.id);
  history.replaceState(null, '', `#${query}`);
}
function report(message) { $('error').textContent = message; $('error').hidden = !message; }
function status() {
  $('loaded').textContent = loaded?.revision || 'none';
  $('latest').textContent = latest?.revision || 'unknown';
  $('notice').textContent = latest && loaded && latest.revision !== loaded.revision
    ? 'New version available — Reload slides' : loaded ? `Loaded ${loaded.revision.slice(0, 12)}` : 'No build loaded';
}
async function manifest() {
  const response = await fetch('/latest.json', { cache: 'no-store', signal: AbortSignal.timeout(10000) });
  if (!response.ok) throw new Error(`Build manifest: HTTP ${response.status}`);
  const data = await response.json();
  if (data.schema !== 1 || !/^[a-f0-9]{64}$/.test(data.revision) || !Array.isArray(data.slides) || !data.slides.length || typeof data.entry !== 'string' || data.entry.startsWith('/') || data.entry.split('/').includes('..')) {
    throw new Error('Invalid build manifest');
  }
  return data;
}
async function detect() {
  if (busy || document.hidden) return;
  const token = ++request;
  try {
    const data = await manifest();
    if (token !== request) return;
    latest = data; status();
  } catch (error) {
    if (token === request) report(`Could not check for updates: ${error.message}. Displayed slides are unchanged.`);
  }
}
async function reload() {
  if (busy) return;
  busy = true; ++request; $('reload').disabled = true; report('');
  let candidate;
  try {
    const at = position();
    latest = await manifest(); status();
    const revision = latest;
    const found = at.id ? revision.slides.indexOf(at.id) : -1;
    const page = found >= 0 ? found + 1 : Math.min(at.page, revision.slides.length);
    const base = `/revisions/${revision.revision}/`;
    const url = new URL(revision.entry, location.origin + base);
    if (url.origin !== location.origin || !url.pathname.startsWith(base)) throw new Error('Invalid build entry');
    const response = await fetch(url, { signal: AbortSignal.timeout(10000) });
    if (!response.ok) throw new Error(`Slides: HTTP ${response.status}`);
    candidate = document.createElement('iframe');
    candidate.className = 'loading'; candidate.title = revision.title || 'Presentation slides';
    await new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error('Timed out loading slides or figures')), 15000);
      candidate.onload = async () => {
        try {
          const doc = candidate.contentDocument;
          if (!doc || doc.querySelectorAll('section').length !== revision.slides.length) throw new Error('Slide document does not match build manifest');
          await Promise.all([...doc.images].map(img => img.decode()));
          for (const css of doc.querySelectorAll('link[rel="stylesheet"]')) {
            if (!css.sheet) throw new Error('A stylesheet did not load');
          }
          clearTimeout(timer); resolve();
        } catch (error) { clearTimeout(timer); reject(error); }
      };
      candidate.onerror = () => { clearTimeout(timer); reject(new Error('Slide document failed to load')); };
      candidate.src = `${url}#${page}`;
      $('slides').append(candidate);
    });
    frame?.remove(); frame = candidate; candidate = null;
    loaded = revision; frame.className = '';
    frame.contentWindow.addEventListener('hashchange', remember);
    $('title').textContent = revision.title || 'Presentation review';
    document.title = $('title').textContent;
    remember(); status();
  } catch (error) {
    candidate?.remove();
    report(`Reload failed: ${error.message}. ${loaded ? 'The previous slides remain displayed.' : 'Try Reload slides again.'}`);
  } finally { busy = false; $('reload').disabled = false; }
}
$('reload').addEventListener('click', reload);
setInterval(detect, 5000); // Announce updates; never interrupt annotations or presenter state.
document.addEventListener('visibilitychange', () => { if (!document.hidden) detect(); });
reload();
