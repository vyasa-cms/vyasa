import "@testing-library/jest-dom/vitest";

// jsdom ships no matchMedia. Components that adapt to viewport width (the
// responsive list, the theme provider) call it on mount, so provide a minimal
// stand-in that always reports "does not match" — i.e. the desktop layout.
if (typeof window !== "undefined" && window.matchMedia === undefined) {
  Object.defineProperty(window, "matchMedia", {
    writable: true,
    value: (query: string) => ({
      matches: false,
      media: query,
      onchange: null,
      addEventListener: () => undefined,
      removeEventListener: () => undefined,
      addListener: () => undefined,
      removeListener: () => undefined,
      dispatchEvent: () => false,
    }),
  });
}
