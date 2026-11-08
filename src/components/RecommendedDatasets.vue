<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { api, type DatasetInfo, type PortalDataset, type ViewInfo } from "../lib/api";
import { loadDatasets } from "../store";
import { installProgress, overall, progress } from "../lib/progress";
import { cancelDownload, confirmDownload, totalBytes, trackDownload, untrackDownload } from "../lib/downloads";

const datasets = ref<PortalDataset[]>([]);
const busy = ref(false);
const loadingMsg = ref("Loading recommended datasets…");
const error = ref("");

const selected = ref<PortalDataset | null>(null);
const views = ref<ViewInfo[]>([]);
const checked = ref<Record<string, boolean>>({});
const viewBusy = ref(false);
const downloading = ref(false);
const downloadId = ref("");
const cancelling = ref(false);
const streamBusy = ref(false);

function generateId(): string {
  return crypto?.randomUUID?.() ?? `r${Date.now()}-${Math.random().toString(36).slice(2)}`;
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
function portalName(ds: PortalDataset): string {
  return ds.datasetRef ?? ds.accession;
}

async function loadRecommended() {
  busy.value = true;
  error.value = "";
  try {
    // Feature the biological collections (e.g. human cancer cell lines) and
    // skip the huge MS-Net reanalysis collection, which otherwise dominates a
    // by-sample-count ranking. Take the most-analyzed datasets per collection.
    const preferred: { name: string; take: number }[] = [
      { name: "celllines", take: 5 },
      { name: "immunopeptidomics", take: 1 },
    ];
    const picked: PortalDataset[] = [];
    const seen = new Set<string>();
    for (const { name, take } of preferred) {
      if (picked.length >= 6) break;
      const res = await api.portalSearch(null, { collection: name, sort: "samples", pageSize: take });
      for (const d of res.datasets) {
        const key = d.datasetRef ?? d.accession;
        if (seen.has(key)) continue;
        seen.add(key);
        picked.push(d);
        if (picked.length >= 6) break;
      }
    }
    datasets.value = picked;
    if (!datasets.value.length) loadingMsg.value = "No featured datasets available.";
  } catch (e: any) {
    error.value = String(e);
    loadingMsg.value = "";
  } finally {
    busy.value = false;
  }
}

async function toggleDataset(ds: PortalDataset) {
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
  viewBusy.value = true;
  error.value = "";
  try {
    const ref = ds.datasetRef ?? ds.accession;
    const v = await api.portalListViews(ref, ds.organism, ds.psmCount);
    if (selected.value === ds) views.value = v;
  } catch (e: any) {
    if (selected.value === ds) error.value = String(e);
  } finally {
    if (selected.value === ds) viewBusy.value = false;
  }
}

function selectedViews() {
  return views.value.filter((v) => checked.value[v.path]);
}
function selectedCount() {
  return selectedViews().length;
}
function selectedBytes() {
  return totalBytes(selectedViews());
}
function toggleAll() {
  const allOn = views.value.every((v) => checked.value[v.path]);
  for (const v of views.value) checked.value[v.path] = !allOn;
}

function openStream(ds: PortalDataset) {
  error.value = "";
  const sel = selectedViews();
  if (!views.value.length) {
    error.value = "This dataset has no listed QPX views to open.";
    return;
  }
  const list = sel.length ? sel : views.value;
  const target: DatasetInfo = {
    prefix: ds.accession,
    path: `quantms portal / ${portalName(ds)}`,
    views: list.map((v) => ({ ...v })).sort((a, b) => a.view.localeCompare(b.view)),
    sourceKind: "portal",
  };
  loadDatasets([target], "portal | " + ds.accession);
}

async function openDownload(ds: PortalDataset) {
  busy.value = true;
  downloading.value = true;
  error.value = "";
  const target = ds;
  try {
    const sel = selectedViews();
    const list = (sel.length ? sel : views.value);
    if (!list.length) throw new Error("This dataset has no listed QPX views to download.");
    if (!(await confirmDownload(list, "portal"))) return;
    if (unmounted || selected.value !== target) return;
    const files = list.map((v) => v.name);
    cancelling.value = false;
    const id = generateId();
    downloadId.value = id;
    trackDownload(id);
    try {
      const local = await api.portalCacheViews(list.map((v) => v.path), id);
      if (unmounted || selected.value !== target) return;
      const nxt: DatasetInfo = {
        prefix: target.accession,
        path: `quantms portal / ${portalName(target)}`,
        views: local.sort((a, b) => a.view.localeCompare(b.view)),
        sourceKind: "portal",
      };
      loadDatasets([nxt], "portal | " + target.accession);
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

onMounted(() => {
  installProgress();
  loadRecommended();
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
  <section class="recommended">
    <div class="rec-head">
      <h3>✨ Recommended datasets</h3>
      <p class="muted">
        Featured biological datasets from the quantms portal — human cancer cell lines and
        immunopeptidomics. Expand one to pick its QPX views, then <b>Stream</b> lazily or
        <b>download to cache</b>.
      </p>
    </div>

    <div v-if="error" class="error">{{ error }}</div>
    <div v-if="busy && !datasets.length" class="muted">
      {{ loadingMsg }} <span class="spinner"></span>
    </div>

    <div v-if="datasets.length" class="grid">
      <article
        v-for="ds in datasets"
        :key="rowKey(ds)"
        class="card"
        :class="{ open: isOpen(ds) }"
      >
        <header class="card-head" @click="toggleDataset(ds)">
          <div class="acc-row">
            <span class="acc">{{ ds.accession }}</span>
            <span v-if="ds.collectionTitle" class="chip" :title="ds.collectionTitle">{{ ds.collectionTitle }}</span>
            <span class="spacer"></span>
            <span class="chev">{{ isOpen(ds) ? "▾" : "▸" }}</span>
          </div>
          <h4>{{ ds.title || ds.accession }}</h4>
          <div class="sub muted">
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

        <div v-if="isOpen(ds)" class="loadbox">
          <div class="toolbar">
            <span class="muted">
              {{ views.length }} view(s) · {{ selectedCount() }} selected<template v-if="selectedCount()"> · {{ fmt(selectedBytes(), true) }}</template>
            </span>
            <div class="actions">
              <button @click="openStream(ds)" :disabled="busy || !views.length">⚡ Stream (lazy)</button>
              <button class="primary" @click="openDownload(ds)" :disabled="busy || !views.length">
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
          <div v-else-if="viewBusy" class="muted">Loading views… <span class="spinner"></span></div>
          <div v-else class="muted">No QPX views listed for this dataset.</div>
        </div>
      </article>
    </div>
  </section>
</template>

<style scoped>
.recommended {
  margin-top: 28px;
  padding-top: 22px;
  border-top: 1px solid var(--border);
  flex: 1 1 auto;
  display: flex;
  flex-direction: column;
  min-height: 0;
}
.rec-head h3 {
  margin: 0 0 4px;
}
.rec-head p {
  margin: 0 0 12px;
  font-size: 12px;
}
.grid {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  grid-auto-rows: minmax(170px, 1fr);
  gap: 12px;
  flex: 1 1 auto;
}
@media (max-width: 1200px) {
  .grid {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }
}
@media (max-width: 720px) {
  .grid {
    grid-template-columns: 1fr;
  }
}
.card {
  border: 1px solid var(--border);
  border-radius: 8px;
  background: var(--bg-soft);
  padding: 12px 14px;
  display: flex;
  flex-direction: column;
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
  max-width: 160px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.spacer {
  flex: 1;
}
.chev {
  color: var(--muted);
}
.card h4 {
  margin: 6px 0 2px;
  font-size: 13px;
}
.sub {
  font-size: 11px;
}
.counts {
  display: flex;
  gap: 14px;
  margin-top: 6px;
  font-size: 11px;
  color: var(--muted);
}
.counts b {
  color: var(--text);
  font-variant-numeric: tabular-nums;
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
  font-size: 12px;
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
</style>
