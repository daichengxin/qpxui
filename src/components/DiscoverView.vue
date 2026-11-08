<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { api, type DatasetInfo, type PortalDataset, type PortalFacets, type PortalSearchResult, type ViewInfo } from "../lib/api";
import { loadDatasets } from "../store";
import { installProgress, overall, progress } from "../lib/progress";
import { cancelDownload, confirmDownload, totalBytes, trackDownload, untrackDownload } from "../lib/downloads";
import PeptideSearchPane from "./PeptideSearchPane.vue";

const TABS = [
  { key: "datasets", label: "Dataset search" },
  { key: "peptides", label: "Peptide & protein search" },
];

const tab = ref<"datasets" | "peptides">("datasets");

const PAGE_SIZE = 50;

const facets = ref<PortalFacets>({ organisms: [], instruments: [], collections: [] });
const results = ref<PortalSearchResult>({ datasets: [], total: 0, page: 1, totalPages: 1, sort: "" });
const q = ref("");
const organism = ref("");
const instrument = ref("");
const collection = ref("");
const sort = ref("peptides");
const page = ref(1);
const busy = ref(false);
const error = ref("");

const selected = ref<PortalDataset | null>(null);
const views = ref<ViewInfo[]>([]);
const checked = ref<Record<string, boolean>>({});
const downloading = ref(false);
const downloadId = ref("");
const cancelling = ref(false);

const SORTS: { key: string; label: string }[] = [
  { key: "peptides", label: "Most peptides" },
  { key: "proteins", label: "Most proteins" },
  { key: "samples", label: "Most samples" },
  { key: "accession", label: "Accession A–Z" },
];

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

function rowKey(ds: PortalDataset): string {
  return ds.datasetRef ?? ds.accession;
}
function isOpen(ds: PortalDataset): boolean {
  return !!selected.value && rowKey(selected.value) === rowKey(ds);
}

async function load() {
  busy.value = true;
  error.value = "";
  try {
    results.value = await api.portalSearch(q.value.trim() || null, {
      collection: collection.value || null,
      organism: organism.value || null,
      instrument: instrument.value || null,
      sort: sort.value || null,
      page: page.value,
      pageSize: PAGE_SIZE,
    });
    // Drop the expanded dataset if it is no longer on this page.
    if (selected.value) {
      const stillThere = results.value.datasets.some(
        (d) => rowKey(d) === rowKey(selected.value!)
      );
      if (!stillThere) {
        selected.value = null;
        views.value = [];
        checked.value = {};
      }
    }
  } catch (e: any) {
    error.value = String(e);
  } finally {
    busy.value = false;
  }
}

function changeFilter() {
  page.value = 1;
  load();
}

let searchTimer: number | undefined;
function onQueryInput() {
  page.value = 1;
  window.clearTimeout(searchTimer);
  searchTimer = window.setTimeout(load, 350);
}

function gotoPage(p: number) {
  if (p < 1 || p > results.value.totalPages || p === page.value) return;
  page.value = p;
  load();
}

function clearFilters() {
  q.value = "";
  organism.value = "";
  instrument.value = "";
  collection.value = "";
  sort.value = "peptides";
  page.value = 1;
  load();
}

async function selectDataset(ds: PortalDataset) {
  if (downloadId.value) cancel();
  if (isOpen(ds)) {
    selected.value = null;
    views.value = [];
    checked.value = {};
    return;
  }
  selected.value = ds;
  views.value = [];
  checked.value = {};
  busy.value = true;
  error.value = "";
  try {
    const ref = ds.datasetRef ?? ds.accession;
    const v = await api.portalListViews(ref, ds.organism, ds.psmCount);
    if (selected.value === ds) views.value = v;
  } catch (e: any) {
    if (selected.value === ds) error.value = String(e);
  } finally {
    if (selected.value === ds) busy.value = false;
  }
}

function portalName(ds: PortalDataset): string {
  return ds.datasetRef ?? ds.accession;
}

function selectedCount() {
  return views.value.filter((v) => checked.value[v.path]).length;
}
function selectedBytes() {
  return totalBytes(views.value.filter((v) => checked.value[v.path]));
}

async function loadSelection() {
  if (!selected.value) return;
  busy.value = true;
  downloading.value = true;
  error.value = "";
  const target = selected.value;
  try {
    const sel = views.value.filter((v) => checked.value[v.path]);
    if (!sel.length) throw new Error("Select at least one view to load.");
    if (!(await confirmDownload(sel, "portal"))) return;
    if (unmounted || selected.value !== target) return;
    const files = sel.map((v) => v.name);
    cancelling.value = false;
    const id = generateId();
    downloadId.value = id;
    trackDownload(id);
    try {
      const local = await api.portalCacheViews(sel.map((v) => v.path), id);
      if (unmounted || selected.value !== target) return;
      const ds: DatasetInfo = {
        prefix: target.accession,
        path: `quantms portal / ${portalName(target)}`,
        views: local.sort((a, b) => a.view.localeCompare(b.view)),
        sourceKind: "portal",
      };
      loadDatasets([ds], "portal | " + target.accession);
    } catch (e: any) {
      if (isCancelledErr(e)) {
        if (!unmounted && selected.value === target) error.value = "Download cancelled; partial files were removed.";
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
    if (selected.value === target) busy.value = false;
    downloading.value = false;
  }
}

function streamSelection() {
  if (!selected.value) return;
  error.value = "";
  const sel = views.value.filter((v) => checked.value[v.path]);
  if (!sel.length) {
    error.value = "Select at least one view to stream.";
    return;
  }
  const ds: DatasetInfo = {
    prefix: selected.value.accession,
    path: `quantms portal / ${portalName(selected.value)}`,
    views: sel.map((v) => ({ ...v })).sort((a, b) => a.view.localeCompare(b.view)),
    sourceKind: "portal",
  };
  loadDatasets([ds], "portal | " + selected.value.accession + " (stream)");
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
function clip(list: string[] | null | undefined, n: number): string[] {
  return (list ?? []).slice(0, n);
}
function doiUrl(doi: string): string {
  return /^https?:\/\//i.test(doi) ? doi : `https://doi.org/${doi}`;
}

onMounted(() => {
  installProgress();
  api.portalFacets().then((f) => (facets.value = f)).catch(() => { /* non-fatal */ });
  load();
});

const hasPrev = computed(() => page.value > 1);
const hasNext = computed(() => page.value < results.value.totalPages);
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
  <div class="discover">
    <div class="head">
      <h3>Discover datasets on the quantms portal</h3>
      <p class="muted">
        Find reanalysed QPX datasets published on the
        <a href="https://portal.quantms.org" target="_blank" rel="noopener">quantms portal</a> — by
        metadata, or by the actual peptides / proteins / genes they contain — then pick an interesting
        one to list its QPX views and <b>Stream</b> them lazily or <b>download to cache</b> for offline
        inspection.
      </p>
    </div>

    <div class="tabbar">
      <button
        v-for="t in TABS"
        :key="t.key"
        class="tab"
        :class="{ on: tab === t.key }"
        @click="tab = t.key as any"
      >
        {{ t.label }}
      </button>
    </div>

    <template v-if="tab === 'datasets'">
    <div class="filters">
      <input
        v-model="q"
        class="search"
        type="search"
        placeholder="Search title, description, keywords, accession…"
        @input="onQueryInput"
      />
      <select v-model="organism" class="sel" @change="changeFilter">
        <option value="">Any organism</option>
        <option v-for="o in facets.organisms" :key="o" :value="o">{{ o }}</option>
      </select>
      <select v-model="instrument" class="sel" @change="changeFilter">
        <option value="">Any instrument</option>
        <option v-for="i in facets.instruments" :key="i" :value="i">{{ i }}</option>
      </select>
      <select v-model="collection" class="sel" @change="changeFilter">
        <option value="">Any collection</option>
        <option v-for="c in facets.collections" :key="c.name" :value="c.name">
          {{ c.title || c.name }}
        </option>
      </select>
      <select v-model="sort" class="sel" @change="changeFilter">
        <option v-for="s in SORTS" :key="s.key" :value="s.key">{{ s.label }}</option>
      </select>
      <button class="ghost" @click="clearFilters">Reset</button>
    </div>

    <div v-if="error && !results.datasets.length" class="error">{{ error }}</div>
    <div v-if="busy && !results.datasets.length" class="muted">
      Searching… <span class="spinner"></span>
    </div>

    <div v-if="results.datasets.length" class="meta muted">
      {{ results.total.toLocaleString() }} datasets · page {{ results.page }} of
      {{ results.totalPages }} · sorted by {{ results.sort || "peptides" }}
    </div>

    <div v-if="results.datasets.length" class="cards">
      <article v-for="ds in results.datasets" :key="rowKey(ds)" class="card" :class="{ open: isOpen(ds) }">
        <header class="card-head" @click="selectDataset(ds)">
          <div class="acc-row">
            <span class="acc">{{ ds.accession }}</span>
            <span v-if="ds.collectionTitle" class="chip" :title="ds.collectionTitle">{{ ds.collectionTitle }}</span>
            <span class="spacer"></span>
            <span v-if="ds.totalSize" class="size muted">{{ fmt(ds.totalSize) }}</span>
            <span class="chev">{{ isOpen(ds) ? "▾" : "▸" }}</span>
          </div>
          <h4>{{ ds.title || ds.accession }}</h4>
          <div class="sub muted" v-if="ds.organism || ds.instrument || ds.samples">
            <span v-if="ds.organism">{{ ds.organism }}</span>
            <span v-if="ds.instrument"> · {{ ds.instrument }}</span>
            <span v-if="ds.samples"> · {{ ds.samples }} samples</span>
          </div>
          <div class="counts">
            <span class="stat" title="Proteins"><b>{{ fmtCount(ds.proteins) }}</b> prot</span>
            <span class="stat" title="Distinct peptides"><b>{{ fmtCount(ds.peptides) }}</b> pep</span>
            <span class="stat" title="PSMs"><b>{{ fmtCount(ds.psmCount) }}</b> PSM</span>
          </div>
        </header>

        <div class="meta-body">
          <p v-if="ds.description" class="desc">{{ ds.description }}</p>
          <div v-if="ds.keywords?.length" class="tags">
            <span v-for="k in clip(ds.keywords, 6)" :key="k" class="tag">{{ k }}</span>
            <span v-if="(ds.keywords?.length ?? 0) > 6" class="tag more">+{{ ds.keywords!.length - 6 }}</span>
          </div>
          <div v-if="ds.tissues?.length" class="tags tissue">
            <span v-for="t in clip(ds.tissues, 5)" :key="t" class="tag">{{ t }}</span>
            <span v-if="(ds.tissues?.length ?? 0) > 5" class="tag more">+{{ ds.tissues!.length - 5 }} tissues</span>
          </div>
          <div class="links">
            <a v-if="ds.pxUrl" :href="ds.pxUrl" target="_blank" rel="noopener">PRIDE ↗</a>
            <a v-if="ds.publication?.doi" :href="doiUrl(ds.publication.doi)" target="_blank" rel="noopener">
              DOI ↗
            </a>
            <span v-if="ds.source" class="muted">{{ ds.source }}</span>
          </div>
        </div>

        <div v-if="isOpen(ds)" class="loadbox">
          <div class="toolbar">
            <span class="muted">
              {{ views.length }} view(s) · {{ selectedCount() }} selected<template v-if="selectedCount()"> · {{ fmt(selectedBytes(), true) }}</template>
            </span>
            <div class="actions">
              <button @click="streamSelection" :disabled="busy || !selectedCount()" title="Query the files in place — each query fetches only the byte ranges it needs">
                ⚡ Stream (lazy)
              </button>
              <button class="primary" @click="loadSelection" :disabled="busy || !selectedCount()">
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
          <div v-else-if="busy" class="muted">Loading views… <span class="spinner"></span></div>
          <div v-else class="muted">No QPX views listed for this dataset.</div>
        </div>
      </article>
    </div>

    <div v-else-if="!busy" class="muted empty">No datasets match your filters.</div>

    <div class="pager" v-if="results.totalPages > 1">
      <button :disabled="!hasPrev" @click="gotoPage(page - 1)">← Prev</button>
      <span class="muted">Page {{ page }} / {{ results.totalPages }}</span>
      <button :disabled="!hasNext" @click="gotoPage(page + 1)">Next →</button>
    </div>
    </template>

    <template v-else-if="tab === 'peptides'">
      <PeptideSearchPane :organisms="facets.organisms" />
    </template>
  </div>
</template>

<style scoped>
.discover {
  border: 1px solid var(--border);
  border-radius: var(--radius);
  padding: 18px;
  background: var(--bg-panel);
}
.head h3 {
  margin: 0 0 6px;
}
a {
  color: var(--accent);
}
.tabbar {
  display: inline-flex;
  gap: 4px;
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 3px;
  margin-bottom: 4px;
}
.tab {
  padding: 6px 14px;
  font-size: 13px;
  border: none;
  background: transparent;
  border-radius: 6px;
}
.tab.on {
  background: var(--accent);
  color: #10142b;
  font-weight: 600;
}
.filters {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  margin: 14px 0 10px;
}
.search {
  flex: 1 1 260px;
  min-width: 200px;
}
.sel {
  max-width: 220px;
}
.ghost {
  padding: 6px 12px;
}
.meta {
  margin: 6px 0 10px;
  font-size: 12px;
}
.cards {
  display: flex;
  flex-direction: column;
  gap: 10px;
  max-height: calc(100vh - 380px);
  overflow-y: auto;
  padding-right: 4px;
}
.card {
  border: 1px solid var(--border);
  border-radius: 8px;
  background: var(--bg-soft);
  padding: 12px 14px;
}
.card.open {
  border-color: var(--accent);
}
.card-head {
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
  max-width: 260px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.spacer {
  flex: 1;
}
.size {
  font-size: 11px;
  white-space: nowrap;
}
.chev {
  color: var(--muted);
}
.card h4 {
  margin: 6px 0 2px;
  font-size: 14px;
}
.sub {
  font-size: 11px;
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
.meta-body {
  margin-top: 8px;
}
.desc {
  font-size: 12px;
  color: var(--text-dim);
  line-height: 1.4;
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
}
.tags {
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
  margin-top: 6px;
}
.tag {
  font-size: 10px;
  padding: 1px 8px;
  border-radius: 999px;
  background: var(--bg-panel);
  border: 1px solid var(--border);
  color: var(--muted);
}
.tag.more {
  color: var(--accent);
}
.tissue .tag {
  font-style: italic;
}
.links {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-top: 8px;
  font-size: 11px;
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
  padding: 14px 0;
  font-size: 12px;
}
.pager {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-top: 12px;
}
</style>
