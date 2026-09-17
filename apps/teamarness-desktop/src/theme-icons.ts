import type { ThemePreference } from "./theme";

const SVG_ATTRIBUTES = 'viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false"';

export const THEME_OPTIONS: readonly Readonly<{ preference: ThemePreference; label: string; icon: string }>[] = Object.freeze([
  {
    preference: "system",
    label: "System theme",
    icon: `<svg ${SVG_ATTRIBUTES}><rect x="3" y="4" width="18" height="12" rx="2"/><path d="M8 20h8M12 16v4"/></svg>`,
  },
  {
    preference: "light",
    label: "Light theme",
    icon: `<svg ${SVG_ATTRIBUTES}><circle cx="12" cy="12" r="4"/><path d="M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M4.93 19.07l1.41-1.41M17.66 6.34l1.41-1.41"/></svg>`,
  },
  {
    preference: "dark",
    label: "Dark theme",
    icon: `<svg ${SVG_ATTRIBUTES}><path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z"/></svg>`,
  },
]);
