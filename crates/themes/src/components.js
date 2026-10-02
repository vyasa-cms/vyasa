/* Progressive enhancement: content stays readable without JavaScript. */
(() => {
  let sequence = 0;
  document.querySelectorAll('.vy-code-tabs').forEach((root) => {
    if (root.dataset.enhanced) return;
    const panels = Array.from(root.querySelectorAll('.vy-code-example'));
    if (!panels.length) return;
    root.dataset.enhanced = 'true';
    const prefix = `vy-code-${++sequence}`;
    const list = document.createElement('div');
    list.setAttribute('role', 'tablist');
    list.setAttribute('aria-label', root.querySelector('h2')?.textContent || 'Code examples');
    const tabs = panels.map((panel, index) => {
      const tab = document.createElement('button');
      tab.type = 'button';
      tab.id = `${prefix}-tab-${index}`;
      tab.setAttribute('role', 'tab');
      tab.setAttribute('aria-controls', `${prefix}-panel-${index}`);
      tab.textContent = panel.querySelector('h3')?.textContent || `Example ${index + 1}`;
      panel.id = `${prefix}-panel-${index}`;
      panel.setAttribute('role', 'tabpanel');
      panel.setAttribute('aria-labelledby', tab.id);
      panel.tabIndex = 0;
      tab.addEventListener('click', () => select(index));
      tab.addEventListener('keydown', (event) => {
        let next;
        if (event.key === 'ArrowRight') next = (index + 1) % panels.length;
        if (event.key === 'ArrowLeft') next = (index + panels.length - 1) % panels.length;
        if (event.key === 'Home') next = 0;
        if (event.key === 'End') next = panels.length - 1;
        if (next !== undefined) { event.preventDefault(); select(next); tabs[next].focus(); }
      });
      list.append(tab);
      if (navigator.clipboard?.writeText) {
        const copy = document.createElement('button');
        copy.type = 'button'; copy.textContent = 'Copy code';
        const feedback = document.createElement('span');
        feedback.className = 'vy-code-feedback'; feedback.setAttribute('role', 'status');
        copy.addEventListener('click', async () => {
          try { await navigator.clipboard.writeText(panel.querySelector('code')?.textContent || ''); feedback.textContent = 'Copied'; }
          catch { feedback.textContent = 'Copy failed. Select and copy the code manually.'; }
        });
        panel.append(copy, feedback);
      }
      return tab;
    });
    function select(index) {
      tabs.forEach((tab, i) => {
        tab.setAttribute('aria-selected', String(i === index)); tab.tabIndex = i === index ? 0 : -1;
        panels[i].hidden = i !== index;
      });
    }
    root.querySelector('.vy-code-examples').prepend(list);
    select(0);
  });
  // Never hide content while waiting for a script or an observer callback.
  if (typeof IntersectionObserver !== 'undefined' && !window.matchMedia('(prefers-reduced-motion: reduce)').matches) {
    const observer = new IntersectionObserver((entries) => {
      entries.forEach((entry) => {
        if (entry.isIntersecting) { entry.target.classList.add('vy-motion-enter'); observer.unobserve(entry.target); }
      });
    }, { threshold: 0.1 });
    document.querySelectorAll('.vy-motion-fade, .vy-motion-rise').forEach((node) => observer.observe(node));
  }
})();
