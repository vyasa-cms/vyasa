/*
 * Resolves the stored theme before first paint so a dark-mode reload never
 * flashes white.
 *
 * This lives in its own file rather than an inline <script> because the server
 * sends `script-src 'self'` — an inline block is blocked outright, which
 * silently reintroduced the flash it was written to prevent. Mirrors
 * THEME_INIT_SCRIPT in src/lib/theme.tsx.
 */
(function () {
  try {
    var stored = localStorage.getItem("vyasa-theme") || "system";
    var dark =
      stored === "dark" ||
      (stored === "system" &&
        window.matchMedia("(prefers-color-scheme: dark)").matches);
    document.documentElement.classList.toggle("dark", dark);
    document.documentElement.style.colorScheme = dark ? "dark" : "light";
  } catch (e) {
    /* storage blocked — fall back to light */
  }
})();
