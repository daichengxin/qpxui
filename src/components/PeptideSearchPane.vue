<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { api, type DatasetInfo, type PsHit, type PsModification, type PsSearchResult, type ViewInfo } from "../lib/api";
import { loadDatasets } from "../store";
import { installProgress, overall, progress } from "../lib/progress";
import { cancelDownload, confirmDownload, totalBytes, trackDownload, untrackDownload } from "../lib/downloads";

defineProps<{ organisms: string[] }>();

const query = ref("");
const explicitType = ref<"auto" | "peptide" | "protein" | "gene">("auto");
const matchMode = ref("exact");
const organism = ref("");
const mod = ref("");
const mods = ref<PsModification[]>([]);

const result = ref<PsSearchResult | null>(null);
const busy = ref(false);
const error = ref("");

const expanded = ref<PsHit | null>(null);
const views = ref<ViewInfo[]>([]);
const checked = ref<Record<string, boolean>>({});
const downloading = ref(false);
const downloadId = ref("");
const cancelling = ref(false);
const viewsBusy = ref(false);

const TYPES: { key: string; label: string }[] = [
  { key: "auto", label: "Auto" },
  { key: "peptide", label: "Peptide" },
  { key: "protein", label: "Protein" },
  { key: "gene", label: "Gene" },
];
const MATCHES = [
  { key: "exact", label: "exact sequence" },
  { key: "contains", label: "contains (substring)" },
];
const showMatch = computed(() => explicitType.value === "auto" || explicitType.value === "peptide");

function rowKey(h: PsHit): string {
  return h.datasetRef || h.datasetAccession;
}
function isExpanded(h: PsHit): boolean {
  return !!expanded.value && rowKey(expanded.value) === rowKey(h);
}

function generateId(): string {
  return crypto?.randomUUID?.() ?? `d${Date.now()}-${Math.random().toString(36).slice(2)}`;
}
function isCancelledErr(e: unknown): boolean {
  return /cancell?/i.test(String(e));
}
function clearProgressFor(files: string[]) {
  for (const f of files) delete progress[f];
}
async function cancel() {
  if (!downloadId.value) return;
  cancelling.value = true;
  await cancelDownload(downloadId.value);
}
let unmounted = false;
onBeforeUnmount(() => {
  unmounted = true;
  void cancelDownload(downloadId.value);
});

async function runSearch() {
  const q = query.value.trim();
  if (!q) return;
  busy.value = true;
  error.value = "";
  expanded.value = null;
  views.value = [];
  checked.value = {};
  try {
    result.value = await api.psSearch(q, {
      dtype: explicitType.value === "auto" ? null : explicitType.value,
      matchMode: explicitType.value === "protein" || explicitType.value === "gene" ? null : matchMode.value || null,
      organism: organism.value || null,
      modification: mod.value || null,
      limit: 50,
    });
  } catch (e: any) {
    error.value = String(e);
  } finally {
    busy.value = false;
  }
}

async function toggleHit(h: PsHit) {
  if (downloadId.value) cancel();
  if (isExpanded(h)) {
    expanded.value = null;
    views.value = [];
    checked.value = {};
    return;
  }
  expanded.value = h;
  views.value = [];
  checked.value = {};
  viewsBusy.value = true;
  error.value = "";
  try {
    const v = await api.portalListViews(h.datasetRef || h.datasetAccession, h.organism, null);
    if (expanded.value === h) views.value = v;
  } catch (e: any) {
    if (expanded.value === h) error.value = String(e);
  } finally {
    if (expanded.value === h) viewsBusy.value = false;
  }
}

function selectedCount() {
  return views.value.filter((v) => checked.value[v.path]).length;
}
function selectedBytes() {
  return totalBytes(views.value.filter((v) => checked.value[v.path]));
}
async function loadSelection(h: PsHit) {
  busy.value = true;
  downloading.value = true;
  error.value = "";
  try {
    const sel = views.value.filter((v) => checked.value[v.path]);
    if (!sel.length) throw new Error("Select at least one view to load.");
    if (!(await confirmDownload(sel, "portal"))) return;
    if (unmounted || expanded.value !== h) return;
    const files = sel.map((v) => v.name);
    cancelling.value = false;
    const id = generateId();
    downloadId.value = id;
    trackDownload(id);
    try {
      const local = await api.portalCacheViews(sel.map((v) => v.path), id);
      if (unmounted || expanded.value !== h) return;
      const ds: DatasetInfo = {
        prefix: h.datasetAccession,
        path: `quantms portal / ${h.datasetRef || h.datasetAccession}`,
        views: local.sort((a, b) => a.view.localeCompare(b.view)),
        sourceKind: "portal",
      };
      loadDatasets([ds], "portal | " + h.datasetAccession);
    } catch (e: any) {
      if (isCancelledErr(e)) {
        if (!unmounted && expanded.value === h) error.value = "Download cancelled; partial files were removed.";
        clearProgressFor(files);
      } else {
        clearProgressFor(files);
        throw e;
      }
    } finally {
      untrackDownload(id);
      if (downloadId.value === id) downloadId.value = "";
      cancelling.value = false;
    }
  } catch (e: any) {
    error.value = String(e);
  } finally {
    if (expanded.value === h) busy.value = false;
    downloading.value = false;
  }
}
function streamSelection(h: PsHit) {
  error.value = "";
  const sel = views.value.filter((v) => checked.value[v.path]);
  if (!sel.length) {
    error.value = "Select at least one view to stream.";
    return;
  }
  const ds: DatasetInfo = {
    prefix: h.datasetAccession,
    path: `quantms portal / ${h.datasetRef || h.datasetAccession}`,
    views: sel.map((v) => ({ ...v })).sort((a, b) => a.view.localeCompare(b.view)),
    sourceKind: "portal",
  };
  loadDatasets([ds], "portal | " + h.datasetAccession + " (stream)");
}
function toggleAll() {
  const allOn = views.value.every((v) => checked.value[v.path]);
  for (const v of views.value) checked.value[v.path] = !allOn;
}

function fmtCount(n: number | null | undefined): string {
  if (n === null || n === undefined) return "—";
  if (n >= 1e6) return `${(n / 1e6).toFixed(n >= 1e7 ? 0 : 1)}M`;
  if (n >= 1e3) return `${(n / 1e3).toFixed(n >= 1e4 ? 0 : 1)}K`;
  return String(n);
}
function fmt(n: number | null | undefined, force = false): string {
  if (!n) return force ? "0 B" : "—";
  const u = ["B", "KB", "MB", "GB", "TB"];
  const i = Math.floor(Math.log(n) / Math.log(1024));
  return `${(n / Math.pow(1024, i)).toFixed(1)} ${u[i]}`;
}

onMounted(async () => {
  installProgress();
  try {
    mods.value = await api.psModifications();
  } catch {
    /* non-fatal */
  }
  runSearch();
});

const activeBars = computed(() =>
  Object.entries(progress)
    .filter(([, b]) => b.phase === "downloading")
    .map(([name, b]) => ({ name, ...b }))
);
const overallPct = computed(() => overall());
function pct(n: number): string {
  return `${Math.max(0, Math.min(100, n)).toFixed(0)}%`;
}
function fmtPx(n: number): string {
  const u = ["B", "KB", "MB", "GB", "TB"];
  const i = Math.floor(Math.log(n) / Math.log(1024));
  return `${(n / Math.pow(1024, i)).toFixed(1)} ${u[i]}`;
}
</script>

<template>
  <div class="ppane">
    <div class="filters">
      <div class="typebar">
        <button
          v-for="t in TYPES"
          :key="t.key"
          class="type"
          :class="{ on: explicitType === t.key }"
          @click="explicitType = t.key as any"
        >
          {{ t.label }}
        </button>
      </div>
      <input
        v-model="query"
        class="search"
        type="search"
        :placeholder="explicitType === 'peptide' ? 'Peptide sequence, e.g. AVGKVLPELNGK…' : 'Peptide, protein or gene — e.g. GAPDH, P04406, AVGKVLPELNGK…'"
        @keydown.enter="runSearch"
      />
      <button class="primary" @click="runSearch" :disabled="busy || !query.trim()">
        {{ busy ? "Searching…" : "Search" }}
      </button>
    </div>

    <div class="subfilters">
      <select v-if="showMatch" v-model="matchMode" class="sel">
        <option v-for="m in MATCHES" :key="m.key" :value="m.key">{{ m.label }}</option>
      </select>
      <select v-model="organism" class="sel">
        <option value="">Any organism</option>
        <option v-for="o in organisms" :key="o" :value="o">{{ o }}</option>
      </select>
      <select v-model="mod" class="sel">
        <option value="">Any modification</option>
        <option v-for="m in mods" :key="m.name" :value="m.name">
          {{ m.name }} ({{ m.nDatasets }} datasets)
        </option>
      </select>
    </div>

    <p class="muted hint">
      Finds which quantms datasets contain the searched peptide, protein or gene across
      the whole portal — then expand a hit to list its QPX views and
      <b>Stream</b> (lazy) or <b>download to cache</b>.
    </p>

    <div v-if="error && !result?.datasets.length" class="error">{{ error }}</div>

    <div v-if="busy && !result" class="muted">Searching… <span class="spinner"></span></div>

    <template v-if="result">
      <div class="meta muted">
        {{ result.totalDatasets.toLocaleString() }} dataset(s)
        <template v-if="expanded || !result.unmapped"> · sorted by number of peptides</template>
      </div>
      <div v-if="result.unmapped" class="note">
        Query didn't map cleanly to a known protein; showing the closest matches.
      </div>

      <div v-if="result.datasets.length" class="hits">
        <article v-for="h in result.datasets" :key="rowKey(h)" class="hit" :class="{ open: isExpanded(h) }">
          <header class="hit-head" @click="toggleHit(h)">
            <div class="acc-row">
              <span class="acc">{{ h.datasetAccession }}</span>
              <span v-if="h.collection" class="chip">{{ h.collection }}</span>
              <span class="spacer"></span>
              <span class="chev">{{ isExpanded(h) ? "▾" : "▸" }}</span>
            </div>
            <div class="sub muted">
              <span v-if="h.organism">{{ h.organism }}</span>
              <span v-if="h.organismPart"> · {{ h.organismPart }}</span>
              <span v-if="h.instrument"> · {{ h.instrument }}</span>
            </div>
            <div class="counts">
              <span class="stat" title="Distinct peptides"><b>{{ fmtCount(h.nPeptides) }}</b> pep</span>
              <span class="stat" title="Peptidoforms"><b>{{ fmtCount(h.nPeptidoforms) }}</b> peptidoforms</span>
              <span class="stat" title="Total observations"><b>{{ fmtCount(h.totalObs) }}</b> obs</span>
            </div>
          </header>

          <div v-if="h.samplePeptidoforms.length && !isExpanded(h)" class="sample">
            <span v-for="(pf, i) in h.samplePeptidoforms.slice(0, 3)" :key="i" class="mono pf">{{ pf }}</span>
          </div>

          <div v-if="isExpanded(h)" class="loadbox">
            <div class="toolbar">
              <span class="muted">
                {{ views.length }} view(s) · {{ selectedCount() }} selected<template v-if="selectedCount()"> · {{ fmt(selectedBytes(), true) }}</template>
              </span>
              <div class="actions">
                <button @click="streamSelection(h)" :disabled="busy || !selectedCount()" title="Query the files in place — each query fetches only the byte ranges it needs">
                  ⚡ Stream (lazy)
                </button>
                <button class="primary" @click="loadSelection(h)" :disabled="busy || !selectedCount()">
                  {{ downloading ? "Downloading…" : "Download to cache" }}
                </button>
              </div>
            </div>

            <div v-if="error" class="error">{{ error }}</div>

            <div v-if="downloading && downloadId" class="progress-box">
              <div class="overall">
                <span class="muted">{{ cancelling ? "Cancelling…" : activeBars.length ? "Transferring to cache…" : "Checking sizes and disk space…" }}</span>
                <span class="right">
                  <span class="muted">{{ overallPct !== null ? pct(overallPct) : "" }}</span>
                  <button class="cancel" @click="cancel" :disabled="!downloadId || cancelling">Cancel</button>
                </span>
              </div>
              <div class="bar"><div class="fill" :style="{ width: pct(overallPct ?? 0) }"></div></div>
              <div v-for="b in activeBars" :key="b.file" class="file-bar">
                <div class="file-row">
                  <span class="mono file-name">{{ b.file }}</span>
                  <span class="muted">{{ fmtPx(b.done) }} / {{ b.total ? fmtPx(b.total) : "…" }} · {{ pct(b.percent) }}</span>
                </div>
                <div class="bar"><div class="fill" :style="{ width: pct(b.percent) }"></div></div>
              </div>
            </div>

            <label class="all">
              <input type="checkbox" :checked="views.length > 0 && selectedCount() === views.length" @change="toggleAll" />
              <span>All views</span>
            </label>
            <div v-if="views.length" class="view-list">
              <label v-for="v in views" :key="v.path" class="view">
                <input v-model="checked[v.path]" type="checkbox" />
                <span class="mono view-name">{{ v.name }}</span>
                <span class="muted">{{ fmt(v.sizeBytes, true) }}</span>
              </label>
            </div>
            <div v-else-if="viewsBusy" class="muted">Loading views… <span class="spinner"></span></div>
            <div v-else class="muted">No QPX views listed for this dataset.</div>
          </div>
        </article>
      </div>
      <div v-else class="muted empty">No datasets match this search.</div>
    </template>
  </div>
</template>

<style scoped>
.ppane {
  display: flex;
  flex-direction: column;
}
.filters {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  align-items: center;
}
.typebar {
  display: inline-flex;
  border: 1px solid var(--border);
  border-radius: 8px;
  overflow: hidden;
}
.type {
  padding: 6px 12px;
  font-size: 12px;
  border: none;
  background: transparent;
}
.type.on {
  background: var(--accent);
  color: #10142b;
  font-weight: 600;
}
.search {
  flex: 1 1 280px;
  min-width: 200px;
}
.subfilters {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  margin-top: 8px;
}
.sel {
  max-width: 260px;
}
.hint {
  font-size: 12px;
  margin: 8px 0;
}
.meta {
  margin: 6px 0 8px;
  font-size: 12px;
}
.note {
  font-size: 12px;
  color: #a06a00;
  margin-bottom: 8px;
}
.hits {
  display: flex;
  flex-direction: column;
  gap: 8px;
  max-height: calc(100vh - 460px);
  overflow-y: auto;
  padding-right: 4px;
}
.hit {
  border: 1px solid var(--border);
  border-radius: 8px;
  background: var(--bg-soft);
  padding: 10px 12px;
}
.hit.open {
  border-color: var(--accent);
}
.hit-head {
  cursor: pointer;
}
.acc-row {
  display: flex;
  align-items: center;
  gap: 8px;
}
.acc {
  font-weight: 700;
  color: var(--accent);
  font-family: var(--mono);
  font-size: 12px;
}
.chip {
  font-size: 10px;
  padding: 1px 8px;
  border-radius: 999px;
  border: 1px solid var(--border);
  color: var(--muted);
}
.spacer {
  flex: 1;
}
.chev {
  color: var(--muted);
}
.sub {
  font-size: 11px;
  margin-top: 2px;
}
.counts {
  display: flex;
  gap: 16px;
  margin-top: 6px;
  font-size: 11px;
  color: var(--muted);
}
.counts b {
  color: var(--text);
  font-variant-numeric: tabular-nums;
}
.sample {
  display: flex;
  flex-wrap: wrap;
  gap: 4px 12px;
  margin-top: 8px;
}
.pf {
  font-size: 10px;
  color: var(--text-dim);
}
.loadbox {
  margin-top: 10px;
  border-top: 1px solid var(--border);
  padding-top: 10px;
}
.toolbar {
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
  font-size: 13px;
  margin-bottom: 8px;
}
.actions {
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
}
.all {
  font-weight: 600;
  font-size: 12px;
  display: flex;
  gap: 8px;
  align-items: center;
  padding-bottom: 6px;
  border-bottom: 1px solid var(--border);
  margin-bottom: 4px;
}
.view-list {
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.view {
  display: flex;
  align-items: center;
  gap: 10px;
  font-size: 12px;
  cursor: pointer;
}
.view-name {
  flex: 1;
}
.error {
  margin: 8px 0;
  color: var(--err);
  font-size: 12px;
}
.progress-box {
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 10px;
  margin: 8px 0;
  background: var(--bg-panel);
}
.overall,
.file-row {
  display: flex;
  justify-content: space-between;
  font-size: 12px;
  margin-bottom: 4px;
}
.right {
  display: inline-flex;
  align-items: center;
  gap: 10px;
}
.cancel {
  padding: 3px 12px;
  font-size: 12px;
  border-color: #d33;
  color: #d33;
}
.file-name {
  max-width: 60%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.bar {
  height: 8px;
  background: var(--border);
  border-radius: 6px;
  overflow: hidden;
  margin-bottom: 8px;
}
.fill {
  height: 100%;
  background: var(--accent);
  border-radius: 6px;
  transition: width 0.15s linear;
}
.file-bar:last-child .bar {
  margin-bottom: 0;
}
.empty {
  padding: 12px 0;
  font-size: 12px;
}
</style>
