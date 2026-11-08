<script setup lang="ts">
import { ref } from "vue";
import { api } from "../lib/api";
import { loadDatasets, state } from "../store";
import S3Picker from "./S3Picker.vue";
import PortalPicker from "./PortalPicker.vue";
import DiscoverView from "./DiscoverView.vue";
import RecommendedDatasets from "./RecommendedDatasets.vue";

const busy = ref(false);
const error = ref("");
const showS3 = ref(false);
const showPortal = ref(false);
const showDiscover = ref(false);

async function run(fn: () => Promise<void>) {
  busy.value = true;
  error.value = "";
  try {
    await fn();
  } catch (e: any) {
    error.value = String(e);
  } finally {
    busy.value = false;
  }
}

async function openSingleFile() {
  await run(async () => {
    const path = await api.chooseFile();
    if (!path) return;
    const ds = await api.openFile(path);
    loadDatasets([ds], "single-file");
  });
}

async function openFolder() {
  await run(async () => {
    const path = await api.chooseFolder();
    if (!path) return;
    const datasets = await api.scanCollection(path);
    if (!datasets.length) {
      // fall back to a flat scan of a data folder with multiple prefixes
      const flat = await api.scanFolder(path);
      if (!flat.length) throw new Error("No QPX parquet files found in this folder.");
      loadDatasets(flat, "dataset");
      return;
    }
    loadDatasets(datasets, "collection");
  });
}
</script>

<template>
  <div class="picker">
    <h2>Open a QPX data source</h2>
    <p class="muted">
      QPX datasets are folders of <span class="mono">&lt;prefix&gt;.&lt;view&gt;.parquet</span> files.
      Open one, browse a collection, or connect to an S3 bucket — everything runs locally.
    </p>

    <div class="grid">
      <button class="card" @click="openSingleFile" :disabled="busy">
        <div class="card-title">📄 Single file</div>
        <div class="card-desc">Open one <span class="mono">*.parquet</span> view and its siblings.</div>
      </button>

      <button class="card" @click="openFolder" :disabled="busy">
        <div class="card-title">🗂️ Folder / collection</div>
        <div class="card-desc">Scan a dataset folder or a folder of many QPX projects.</div>
      </button>

      <button class="card" @click="showS3 = !showS3" :disabled="busy">
        <div class="card-title">☁️ S3 bucket</div>
        <div class="card-desc">Browse and query projects hosted in a remote S3 bucket.</div>
      </button>

      <button class="card" @click="showPortal = !showPortal" :disabled="busy">
        <div class="card-title">🌍 Quantms portal</div>
        <div class="card-desc">Open a reanalysed dataset from the public quantms portal over HTTPS.</div>
      </button>

      <button class="card" @click="showDiscover = !showDiscover" :disabled="busy">
        <div class="card-title">🔎 Discover quantms data</div>
        <div class="card-desc">Search and filter datasets by text, organism, instrument or collection to find interesting QPX files.</div>
      </button>

      <div class="card info">
        <div class="card-title">🔬 What is QPX?</div>
        <div class="card-desc">
          An open quantitative-proteomics format: a folder of
          <span class="mono">&lt;prefix&gt;.&lt;view&gt;.parquet</span> views (peptides, proteins,
          PSMs, …) queryable in place — no database needed.
        </div>
      </div>
    </div>

    <div class="err" v-if="error">⚠ {{ error }}</div>
    <div v-if="busy" class="muted">Working… <span class="spinner"></span></div>
    <div v-if="state.error">{{ state.error }}</div>

    <S3Picker v-if="showS3" style="margin-top: 20px" />
    <PortalPicker v-if="showPortal" style="margin-top: 20px" />
    <DiscoverView v-if="showDiscover" style="margin-top: 20px" />

    <RecommendedDatasets v-if="!showS3 && !showPortal && !showDiscover" />
  </div>
</template>

<style scoped>
.picker {
  max-width: 1680px;
  margin: 0 auto;
  padding: 28px 32px;
  min-height: 100%;
  display: flex;
  flex-direction: column;
}
h2 {
  margin: 0 0 6px;
}
.grid {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  grid-auto-rows: minmax(112px, auto);
  gap: 14px;
  margin-top: 20px;
  flex: 0 0 auto;
}
@media (max-width: 900px) {
  .grid {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }
}
@media (max-width: 560px) {
  .grid {
    grid-template-columns: 1fr;
  }
}
.card {
  text-align: left;
  padding: 16px 18px;
  min-height: 112px;
  background: var(--bg-soft);
  display: flex;
  flex-direction: column;
  gap: 8px;
}
.card.info {
  cursor: default;
}
.card.info .card-desc {
  line-height: 1.5;
}
.card-title {
  font-weight: 700;
  font-size: 15px;
}
.card-desc {
  color: var(--text-dim);
  font-size: 13px;
  line-height: 1.4;
}
.err {
  margin-top: 18px;
  color: var(--err);
}
</style>
