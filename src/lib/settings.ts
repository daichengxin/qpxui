// App settings: light/dark theme + per-PTM custom colours.
// Persisted to localStorage (survives restarts in the Tauri webview).
import { reactive } from "vue";

export type Theme = "dark" | "light";

export interface PTMEntry {
  /** Stable id used as the settings key. */
  id: string;
  /** Human label shown in the settings panel. */
  label: string;
  /** CV accessions that map to this modification (e.g. UNIMOD:21). */
  accessions: string[];
  /** Name substrings (lowercased) used to match non-accession names. */
  keywords: string[];
  /** Default colour when the user hasn't overridden it. */
  default: string;
}

export const PTM_PALETTE: PTMEntry[] = [
  { id: "phospho", label: "Phosphorylation", accessions: ["UNIMOD:21"], keywords: ["phospho"], default: "#f59e0b" },
  { id: "oxidation", label: "Oxidation", accessions: ["UNIMOD:35"], keywords: ["oxid", "methion", "hydroxyl"], default: "#22c55e" },
  { id: "acetyl", label: "Acetylation", accessions: ["UNIMOD:1"], keywords: ["acetyl"], default: "#a78bfa" },
  { id: "carbamidomethyl", label: "Carbamidomethyl", accessions: ["UNIMOD:4"], keywords: ["carbamidomethyl"], default: "#f97316" },
  { id: "carbamyl", label: "Carbamyl / Carbamylation", accessions: ["UNIMOD:5"], keywords: ["carbamyl"], default: "#fb923c" },
  { id: "deamidated", label: "Deamidated", accessions: ["UNIMOD:7"], keywords: ["deamid", "deamidation"], default: "#e879e9" },
  { id: "methyl", label: "Methylation", accessions: ["UNIMOD:34"], keywords: ["methyl", "methylation"], default: "#2dd4b6" },
  { id: "dimethyl", label: "Dimethyl", accessions: ["UNIMOD:36"], keywords: ["dimethyl", "dethiomethyl"], default: "#14b8a6" },
  { id: "trimethyl", label: "Trimethyl", accessions: ["UNIMOD:37"], keywords: ["trimethyl"], default: "#0d9488" },
  { id: "glygly", label: "GlyGly / Ubiquitination", accessions: ["UNIMOD:121"], keywords: ["gly", "ubiquit"], default: "#38bdf8" },
  { id: "sulfation", label: "Sulfation", accessions: ["UNIMOD:39"], keywords: ["sulfo", "sulf"], default: "#24d3cc" },
  { id: "guanidinyl", label: "Guanidinyl", accessions: ["UNIMOD:43"], keywords: ["guanidin"], default: "#a78bfa" },
  { id: "isobaric", label: "Isobaric labels (TMT / iTRAQ / SILAC)", accessions: [], keywords: ["tmt", "itraq", "silac", "label:", "isotopic", "icat", "chelator", "piol"], default: "#60a5fa" },
  { id: "unknown", label: "Unknown / mass-shift", accessions: [], keywords: [], default: "#94a3b8" },
];

const STORAGE_KEY = "qpxui.settings.v1";

export interface SettingsState {
  theme: Theme;
  /** entry.id -> colour override hex. */
  ptmColors: Record<string, string>;
}

function defaults(): SettingsState {
  return { theme: "light", ptmColors: {} };
}

function load(): SettingsState {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (raw) return { ...defaults(), ...JSON.parse(raw) };
  } catch {
    /* fall through to defaults */
  }
  return defaults();
}

export const settings = reactive<SettingsState>(load());

export function persist() {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(settings));
  } catch {
    /* storage unavailable — ignore */
  }
}

/** Push the current theme onto <html> so the CSS variables switch. */
export function applyTheme() {
  const root = document.documentElement;
  root.dataset.theme = settings.theme;
  root.style.colorScheme = settings.theme;
}

export function setTheme(t: Theme) {
  settings.theme = t;
  applyTheme();
  persist();
}

export function setPtmColor(id: string, hex: string) {
  settings.ptmColors[id] = hex;
  persist();
}

/** Effective (user-overridden or default) colour for a palette entry. */
export function effectivePtmColor(id: string): string {
  return settings.ptmColors[id] ?? PTM_PALETTE.find((e) => e.id === id)!.default;
}

/** Reset one palette entry back to its default colour. */
export function resetPtmColor(id: string) {
  delete settings.ptmColors[id];
  persist();
}

/**
 * Resolve the colour shown for a modification, honouring user overrides.
 * 1) match by CV accession, 2) match by name keyword, 3) unknown fallback.
 */
export function resolvePtmColor(name?: string, accession?: string): string {
  const acc = (accession || "").toUpperCase();
  if (acc && PTM_PALETTE.some((e) => e.accessions.includes(acc))) {
    const entry = PTM_PALETTE.find((e) => e.accessions.includes(acc))!;
    return effectivePtmColor(entry.id);
  }
  const n = (name || "").toLowerCase();
  if (n.trim()) {
    const byKw = PTM_PALETTE.find((e) => e.keywords.some((k) => n.includes(k)));
    if (byKw) return effectivePtmColor(byKw.id);
  }
  return effectivePtmColor("unknown");
}
