# QPX UI

[![CI](https://github.com/bigbio/qpxui/actions/workflows/ci.yml/badge.svg)](https://github.com/bigbio/qpxui/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/bigbio/qpxui?include_prereleases)](https://github.com/bigbio/qpxui/releases/latest)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue)](LICENSE)

A **desktop viewer for [QPX](https://github.com/bigbio/qpx) (Quantitative Proteomics eXchange)
data**. You can open a local file or folder, an S3 bucket, or any reanalysed dataset on the
[quantms portal](https://portal.quantms.org). Each dataset gets a report with QC charts and
statistics, plus protein and peptide search, a data browser and a SQL console.

QPX datasets are folders of `<prefix>.<view>.parquet` files (`PXD014414.feature.parquet`,
`PXD014414.pg.parquet`, `PXD014414.run.parquet`, …). QPX UI runs [DuckDB](https://duckdb.org)
inside the app, so every query runs on your machine. Remote files are **streamed lazily**: only
the parquet footer and the column chunks a query needs are fetched, never the whole file.

Built with [Tauri v2](https://v2.tauri.app/) (Rust), Vue 3 + TypeScript + Vite, `duckdb-wasm` and ECharts.

![Dataset report for PXD014414: headline counts and identification QC](docs/screenshots/report-overview.png)

---

## Contents

- [Install](#install)
- [Quick start](#quick-start)
- [Features](#features)
  - [Data sources](#data-sources)
  - [Dataset report](#dataset-report)
  - [Quantification](#quantification)
  - [Protein & peptide search](#protein--peptide-search)
  - [Spectrum viewer](#spectrum-viewer)
  - [Browse, summary & SQL](#browse-summary--sql)
  - [PTMs, settings & export](#ptms-settings--export)
- [Streaming from S3 and the portal](#streaming-from-s3-and-the-portal)
- [Development](#development)
- [Architecture](#architecture)
- [Releasing](#releasing)
- [Limitations](#limitations)
- [License](#license)

---

## Install

Download the installer for your platform from the
[latest release](https://github.com/bigbio/qpxui/releases/latest):

| Platform | File | Notes |
|----------|------|-------|
| macOS, Apple Silicon | `qpxui_<version>_aarch64.dmg` | Open the `.dmg` and drag **qpxui** to Applications. |
| macOS, Intel | `qpxui_<version>_x64.dmg` | Same as above. |
| Windows 10/11 | `qpxui_<version>_x64-setup.exe` or `.msi` | Uses the WebView2 runtime that ships with Windows. |
| Linux | `.AppImage`, `.deb`, `.rpm` | `chmod +x qpxui*.AppImage && ./qpxui*.AppImage`, or install the package. |

> **macOS: the app is not notarized yet.** The first launch may say the app "is damaged" or
> "can't be opened". Right-click the app → **Open**, or run
> `xattr -dr com.apple.quarantine /Applications/qpxui.app`.

---

## Quick start

1. Launch QPX UI and pick a source:
   - **🌍 Quantms portal**: pick a reanalysed dataset (for example `PXD014414`), then press **⚡ Stream**.
   - **🗂️ Folder / collection**: a folder with one dataset, or with many `PXD*/` dataset folders.
   - **📄 Single file**: one `*.parquet` view; the files next to it are picked up too.
   - **☁️ S3 bucket**: a bucket/prefix, read with your AWS credentials.
2. The **dataset report** opens. Browse the tabs, or type a protein, gene or peptide into the
   search box in the sidebar.
3. Click any view in the sidebar (`feature`, `psm`, `pg`, …) to browse its rows, chart its columns,
   or query it with SQL.

---

## Features

### Data sources

| Source | What it does |
|--------|--------------|
| **Single file** | Opens one `*.parquet` view plus its sibling views from the same folder. |
| **Folder / collection** | Opens one QPX dataset folder, or a folder tree holding many projects. |
| **S3 bucket** | Lists datasets under a bucket/prefix using your AWS credentials. You can stream the files lazily or download them to a local cache. S3-compatible stores (MinIO, Ceph) work too. |
| **Quantms portal** | Searchable list of the datasets on [portal.quantms.org](https://portal.quantms.org) (title, organism, instrument, counts). You can stream a dataset lazily or download it to a local cache. No account needed. |

Downloads show progress and can be cancelled at any point, even in the middle of a file; partial
files are removed. Before a download starts, the app checks the free space on the cache disk: it
refuses a download that would leave less than 1 GB free, and asks before downloading more than
2 GB. Choosing **New source** or another dataset cancels a running download and stops the previous
dataset's queries.

While you stream, the sidebar shows how much of each file has actually been fetched (for example
`⚡ 16 MB / 170 MB`).

### Dataset report

Selecting a dataset that has a `feature`, `psm` or `pg` view opens its report. It works like the
portal's Dataset Explorer, but runs locally on any QPX data. You can also reach it from
**📈 Report & search** in the sidebar.

- **Header**: title, description, organisms, instruments, enzymes, software, and PRIDE/PubMed
  links. **Cite this data** copies a citation or saves it as BibTeX or RIS.
- **Headline counts**: protein groups, peptides, features, PSMs, runs and samples.

The report has these tabs. A tab only appears when the dataset has the data for it:

| Tab | Contents |
|-----|----------|
| **Overview** | Identifications per run. Distributions of precursor charge, peptide length, missed cleavages, peptides per protein group, precursor mass error (ppm), retention time, intensity, posterior error probability and peptide q-value. |
| **QC** | Identifications per run (features, peptides, proteins, protein groups). Median intensity and median mass error per run, to spot loading or calibration drift. Intensity distribution per sample (TMT channels are matched to samples through the run view). A per-run table with sample, fraction and instrument. |
| **PTMs** | How often each modification occurs (counted by peptidoforms, peptides or features), drawn in your PTM colours. Modified sites by residue, modifications per peptide, and a table; click a modification to list its peptides. |
| **Samples** | Charts of the sample metadata (organism, tissue, disease, cell line, sex, …) and a filterable sample table showing which runs and channels each sample was measured in. |
| **Proteins / Peptidoforms** | Search; see below. |
| **Quantification** | Protein × sample abundance matrix, sample correlation, PCA and per-protein profiles; see below. |

Statistics are computed over target rows only (decoys left out). Histograms show the central 99%
of values, and the mass-error histogram also drops extreme outliers such as isotope-error matches.
Results are cached, so switching tabs is instant.

![QC tab: identifications, median intensity and mass error per run, intensity per sample](docs/screenshots/report-qc.png)

### Quantification

The **Quantification** tab appears when the `pg` view (or, failing that, the `feature` view) has
`intensities`. It builds a protein × sample matrix of log2 abundances:

- **Source**: protein-group intensities from `pg`, or feature intensities summed per anchor protein.
- **Columns**: run channels are mapped to samples through the run view's `samples` (TMT/iTRAQ
  channels are matched by label, fractions are summed); otherwise there is one column per run.
- **Options**: median normalisation, include/exclude contaminants, the minimum fraction of samples
  a protein must be quantified in, and **colour by** a sample attribute (disease, tissue, …).
- **Charts**: abundance distribution and proteins quantified per sample; a Pearson **sample
  correlation** heatmap ordered by average-linkage clustering; **PCA** with selectable components and
  variance explained.
- **Abundance profile**: pick a protein from the table to see its abundance in every sample,
  coloured by group, with group means.
- **Export**: every chart as PNG/CSV, and the whole log2 matrix as CSV (**Matrix CSV**).

![Quantification tab: sample correlation, PCA coloured by disease and a protein abundance profile](docs/screenshots/quantification.png)

### Protein & peptide search

- **Proteins**: search by accession, protein name or gene name (for example `P04637`, `ALBU`, `EGFR`).
  The search runs on the protein-group (`pg`) view when there is one, and otherwise on the
  proteins in the feature or PSM view. Clicking a protein shows:
  - UniProt links, peptidoform, peptide and feature counts;
  - a per-run evidence chart;
  - its PTM-coloured peptidoforms;
  - its protein group in each run.
- **Peptidoforms**: search by sequence (*contains*, *starts with* or *exact*), or by peptidoform text
  such as `M[Oxidation]` or `UNIMOD:21`. Filters for modification and minimum charge are available.
  Clicking a peptidoform opens its **Profile**: its intensity in every sample (runs and TMT channels
  mapped to samples through the run view), the distribution per sample group, and its retention time
  in each run. The **List** pane shows every observation (run, charge, RT, m/z, ppm, PEP, intensity),
  and its protein links open the protein.

![Protein view: per-run evidence, PTM-coloured peptidoforms and the protein group in each run](docs/screenshots/protein.png)

- The **sidebar search box** searches the selected dataset. An upper-case amino-acid string of 8 or
  more residues is searched as a peptide.

### Spectrum viewer

When the PSM view stores fragment peaks (`mz_array` / `intensity_array`, as in the
[π-MSNet](https://github.com/PHOENIXcenter/pi-MSnet) collection), the peptide evidence pane lists
PSMs, and clicking one opens an annotated spectrum in the style of the PRIDE USI viewer:

- stick plot with b (blue) and y (red) ions, neutral losses and labels; zoom with the wheel or slider;
- fragment coverage map, mass-error panel and a b/y fragment ion table;
- annotations either **from the file** (spectrum_utils labels stored in `ion_type_array`) or
  **computed** here, with an adjustable tolerance (ppm or Da), fragment charge and H₂O/NH₃ losses;
- the PSM's USI (`mzspec:PXD…:run:scan:N:peptidoform/z`), with a link to the PRIDE USI viewer;
- **mirror spectra** to check an identification against a reference drawn upside down:
  - another PSM of the same peptidoform;
  - a predicted spectrum from [Koina](https://koina.wilhelmlab.org) (Prosit 2020 HCD,
    AlphaPeptDeep, MS²PIP; the model is picked from the peptidoform's modifications);
  - any spectrum by USI, fetched from PRIDE PROXI (ProteomeCentral as fallback);
  - the theoretical b/y ladder.

  Matched peaks are scored with the cosine of square-root intensities and the normalised spectral
  angle;
- PNG and CSV (peaks + annotations) export.

![Spectrum viewer: annotated PSM mirrored against a Prosit prediction (cosine 0.71)](docs/screenshots/spectrum-mirror.png)

Predictions and USI lookups need internet access; everything else is local.

Only the selected PSM's row is read, so this also works on large streamed files. Whether a file
has spectra is decided from the parquet footer statistics. π-MSNet file names
(`<prefix>-MSNet.parquet`, `<prefix>-MSNet.psm.parquet`) are recognised as the PSM view of `<prefix>`.

### Browse, summary & SQL

Click a view in the sidebar to work with its rows directly:

- **Browse**: a table showing 500 rows per page, with column sorting and a live SQL `WHERE` filter.
  Nested cells (lists and structs) can be expanded.
- **Summary**: automatic charts per column, and a histogram for any numeric column.
- **Query**: SQL (DuckDB dialect, the same engine as `qpxc query sql`) against the registered
  views, with example queries to start from.

### PTMs, settings & export

- Peptides and peptidoforms are colour-coded by modification, with short codes and tooltips
  (name, CV accession, residue, position).
- **⚙ Settings**: dark or light theme, and a colour for each PTM, which every table and chart uses.
- **Export**: every chart can be saved as PNG or CSV, and every table as CSV, through the system
  save dialog.

---

## Streaming from S3 and the portal

**⚡ Stream** queries files where they are stored, without downloading them:

- The Rust backend runs a local HTTP range proxy on `127.0.0.1` (`src-tauri/src/commands/s3stream.rs`).
- DuckDB reads only the parquet footer plus the column chunks and row groups each query touches.
- The proxy fetches just those byte ranges, in 4 MB requests, 8 in parallel. It keeps them in a
  512 MB in-memory cache of 256 KB blocks, so repeated queries don't refetch.
- **S3**: requests are signed with the default AWS credential chain (environment variables,
  `~/.aws/credentials`, SSO profile or IAM role). Credentials never reach the webview, and buckets
  need no CORS setup.

  ```bash
  AWS_PROFILE=myprofile AWS_REGION=eu-west-1 open -a qpxui    # macOS
  AWS_ENDPOINT_URL=http://localhost:9000 qpxui                # MinIO / S3-compatible
  ```
- **Portal**: the proxy follows the portal's redirect to a short-lived signed storage URL, and gets
  a new one when it expires.

Portal HTTPS requests use the operating system's certificate verifier and trust store, including
system-provided intermediates on macOS and Windows. If your company installs its CA in the system
store, no extra configuration should be needed.

Alternatively, set `SSL_CERT_FILE` to a PEM certificate bundle and/or `SSL_CERT_DIR` to directories
of PEM certificates before starting qpxui. As in uv, non-empty settings replace the default trust
source: include every CA needed for portal and storage hosts. Separate multiple directories with
`:` on Unix or `;` on Windows. Configured paths that cannot be read, or configured sources that
collectively contain no certificates, cause an error rather than falling back to system trust.

```bash
SSL_CERT_FILE=/path/to/ca-bundle.pem qpxui
```

On macOS, launch the executable directly (for example,
`SSL_CERT_FILE=/path/to/ca-bundle.pem /Applications/qpxui.app/Contents/MacOS/qpxui`);
on Windows PowerShell, set `$env:SSL_CERT_FILE` before launching the executable.
Only use certificates from a trusted source. Restart the app after changing the configuration.
These settings apply to portal registry, listing, downloads and streaming; the separate S3 client
may handle certificate configuration differently. HTTPS certificate verification remains enabled.

**Download to cache** instead fetches whole files once (with progress and cancel), for fully
offline use. Cached files are kept in the OS cache folder (`~/Library/Caches/qpxui/` on macOS,
`~/.cache/qpxui/` on Linux, `%LOCALAPPDATA%\qpxui\` on Windows).

### Large streamed files

Opening a streamed file of 1 GB or more reads only its parquet footer, so it opens in seconds
however big it is. Anything that would read whole columns waits for you to ask:

- **Header**: PSM and feature counts come from the footer; protein, peptide and run counts show a
  **count** button.
- **Overview, QC, PTMs, Quantification**: a **Compute** button shows a lower-bound estimate of the
  data it will fetch.
- **Proteins and Peptidoforms**: wait for a search term; peptide search defaults to an exact sequence match.
- **Browse**: peak arrays start hidden; **Summary** builds its charts on request.

How fast searches and spectra are depends on how the file was written: files sorted by peptidoform
with small row groups (`qpxc transform relayout`) let each lookup read a few MB.

---

## Development

**Prerequisites**

- Node.js 20+ and npm
- Rust (stable) via [rustup](https://rustup.rs)
- Tauri system dependencies:
  - macOS: Xcode Command Line Tools (`xcode-select --install`);
  - Windows: [WebView2](https://developer.microsoft.com/microsoft-edge/webview2/) and MSVC build tools;
  - Linux: `libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev libssl-dev patchelf`
    (see the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/)).

**Run and build**

```bash
npm install
npm run tauri dev      # run the app with hot reload (also compiles the Rust backend)
npm run tauri build    # installers for this platform → src-tauri/target/release/bundle/
```

**Checks** (the same ones CI runs on every PR):

```bash
npx vue-tsc --noEmit           # type-check the frontend
npx vite build                 # build the frontend
(cd src-tauri && cargo check)  # check the backend
```

**Sample data**

```bash
node scripts/fetch-sample.mjs   # small example feature/psm views → ./sample
pip install "qpx[all]"          # or convert your own search results to QPX
qpxc convert diann --report-path report.tsv --sdrf-file data.sdrf.tsv --output-folder ./sample/PXD000000
```

Many reanalysed datasets (e.g. `PXD014414`) can be opened directly with the **Quantms portal** source.

---

## Architecture

```
┌───────────────────────────────────────────────────────────────────┐
│ WebView: Vue 3 + duckdb-wasm + ECharts                            │
│  SourcePicker · PortalPicker · S3Picker                           │
│  DatasetExplorer (sidebar, search box)                            │
│  ReportView ─ Overview · QC · PTMs · Samples · Proteins ·         │
│               Quantification · Peptidoforms · SpectrumViewer      │
│  ViewPanel  ─ Browse · Summary · Query                            │
│        all SQL runs here, in DuckDB (wasm)                        │
└──────────────┬─────────────────────────────────┬──────────────────┘
               │ Tauri IPC                        │ HTTP range reads
┌──────────────▼─────────────────────────────────▼──────────────────┐
│ Rust backend                                                      │
│  sources.rs   QPX discovery (file / folder / collection)          │
│  io.rs        read local files, write exports                     │
│  s3.rs        list / download S3 objects (aws-sdk-s3, rustls)     │
│  portal.rs    quantms portal registry, listing, downloads         │
│  s3stream.rs  127.0.0.1 range proxy with block cache (S3 + HTTP)  │
│  transfers.rs progress events and cancellation                    │
└───────────────────────────────────────────────────────────────────┘
```

Report queries are built in `src/lib/report.ts`:

- They select only the columns they need and adapt to each dataset's schema: `anchor_protein`
  vs `protein_accessions`, a struct- or list-typed `pg_accessions`, and LFQ or TMT `intensities`.
- If the `sequence` column contains modification names, the plain sequence is derived from the
  peptidoform instead.

<details>
<summary>Project layout</summary>

```
qpxui/
├── src/
│   ├── App.vue                    # shell: top bar, sidebar, main panel
│   ├── store.ts                   # app state (datasets, page, report tab, search)
│   ├── lib/
│   │   ├── api.ts                 # Tauri IPC wrappers + types
│   │   ├── duck.ts                # DuckDB init, view registration, queries
│   │   ├── report.ts              # report SQL helpers, dataset table registry, search
│   │   ├── charts.ts              # ECharts option builder, CSV export
│   │   ├── quant.ts               # normalisation, correlation, clustering, PCA
│   │   ├── fragments.ts           # peptidoform parsing, fragment ions, annotation
│   │   ├── mirror.ts              # Koina predictions, PROXI/USI, spectrum similarity
│   │   ├── export.ts              # save dialog helpers (PNG / CSV / BibTeX)
│   │   ├── ptm.ts, settings.ts    # PTM parsing/colours, theme + settings
│   │   └── useSlots.ts            # sequential chart loading with progress
│   └── components/
│       ├── ReportView.vue         # report header, counts, citation, tabs
│       ├── report/                # Overview, QC, PTMs, Samples, Proteins, Quant, Peptides
│       ├── SpectrumViewer.vue     # annotated spectrum + mirror
│       ├── ChartCard.vue, ReportTable.vue, PeptideSeq.vue
│       ├── SourcePicker.vue, PortalPicker.vue, S3Picker.vue
│       ├── DatasetExplorer.vue    # sidebar: search, datasets, views
│       ├── ViewPanel.vue, DataTable.vue, SummaryView.vue, QueryView.vue
│       └── SettingsPanel.vue, StructuredView.vue, PTMTip.vue
├── src-tauri/
│   ├── tauri.conf.json, Cargo.toml, capabilities/
│   └── src/{lib.rs, qpx/, commands/}
├── scripts/                       # fetch-sample.mjs, make-icon.py
└── .github/workflows/             # ci.yml (PR checks), release.yml (installers)
```
</details>

---

## Releasing

1. Bump the version in `package.json`, `src-tauri/tauri.conf.json` and `src-tauri/Cargo.toml`.
2. Merge to `main`, then push a tag:

   ```bash
   git tag v0.2.0 && git push origin v0.2.0
   ```

3. The **Release** workflow builds:
   - macOS arm64 and x64 (both on Apple Silicon runners; x64 is cross-compiled);
   - Windows x64;
   - Linux x64.

   It attaches the installers to a **draft** release. Review the draft and publish it.

App icons are generated from `src-tauri/icons/icon-source.png` (drawn by `scripts/make-icon.py`)
with `npx tauri icon src-tauri/icons/icon-source.png`.

---

## Limitations

- AnnData/HDF5 matrices (`.ae.h5ad`, `.de.h5ad`, `.h5mu`) are listed but not yet rendered; the
  viewer focuses on the Parquet views.
- Local files are loaded into the webview's memory when first queried. Very large local views
  (e.g. spectra in `mz`) may hit the limits of DuckDB-wasm, so streaming is preferable for big
  remote files.
- The builds are not code-signed yet (see the macOS note under [Install](#install)).

## License

[Apache-2.0](LICENSE)
