/** This Package's own CSS, adopted into the mount's shadow root and removed on unmount. */
export const PACKAGE_STYLES = `
  :host { color: var(--teamarness-color-text); font-family: var(--teamarness-font-body); }
  article { display: grid; gap: .7rem; padding: 1rem; }
  h1, p { margin: 0; }
  .reads { color: var(--teamarness-color-text-muted); }
  .error { color: var(--teamarness-color-danger); font-family: var(--teamarness-font-mono); }
`;
