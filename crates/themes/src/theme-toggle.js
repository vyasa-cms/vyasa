/* Colour-scheme toggle. Loaded as a blocking script in <head>, so a stored
   choice is applied before first paint (no flash). Any element with
   data-vy-theme-toggle switches light/dark and remembers the choice. */
(() => {
  const KEY = 'vy-theme';
  const root = document.documentElement;
  const read = () => { try { return localStorage.getItem(KEY); } catch (_) { return null; } };
  const write = (v) => { try { localStorage.setItem(KEY, v); } catch (_) { /* private mode */ } };
  const stored = read();
  if (stored === 'light' || stored === 'dark') root.setAttribute('data-theme', stored);
  const current = () => root.getAttribute('data-theme')
    || (matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light');
  document.addEventListener('click', (event) => {
    const button = event.target instanceof Element && event.target.closest('[data-vy-theme-toggle]');
    if (!button) return;
    const next = current() === 'dark' ? 'light' : 'dark';
    root.setAttribute('data-theme', next);
    write(next);
    button.setAttribute('aria-pressed', String(next === 'dark'));
  });
})();
