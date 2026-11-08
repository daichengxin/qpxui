import { invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";

export interface ViewInfo {
  name: string;
  view: string;
  format: string;
  path: string;
  sizeBytes: number;
}

export interface DatasetInfo {
  prefix: string;
  path: string;
  views: ViewInfo[];
  sourceKind: string;
}

export interface S3Object {
  key: string;
  sizeBytes: number;
  lastModified: string | null;
}

export interface S3Listing {
  bucket: string;
  prefix: string;
  objects: S3Object[];
  datasets: DatasetInfo[];
}

export interface PortalDataset {
  accession: string;
  title?: string | null;
  organism?: string | null;
  instrument?: string | null;
  samples?: number | null;
  proteins?: number | null;
  peptides?: number | null;
  psmCount?: number | null;
  totalSize?: number | null;
  datasetRef?: string | null;
  pxUrl?: string | null;
  collectionTitle?: string | null;
  description?: string | null;
  summary?: string | null;
  keywords?: string[] | null;
  diseases?: string[] | null;
  tissues?: string[] | null;
  publication?: { doi?: string | null } | null;
  source?: string | null;
  count?: number | null;
  collection?: string | null;
}

export interface PortalSearchResult {
  datasets: PortalDataset[];
  total: number;
  page: number;
  totalPages: number;
  sort: string;
}

export interface CollectionOption {
  name: string;
  title?: string | null;
  datasetCount?: number | null;
}

export interface PortalFacets {
  organisms: string[];
  instruments: string[];
  collections: CollectionOption[];
}

export interface PsHit {
  datasetAccession: string;
  datasetRef: string;
  collection?: string | null;
  organism?: string | null;
  organismPart?: string | null;
  disease?: string | null;
  instrument?: string | null;
  datasetUrl: string;
  pxUrl?: string | null;
  nPeptidoforms: number;
  nPeptides: number;
  totalObs: number;
  samplePeptidoforms: string[];
}

export interface PsFacetValue {
  value: string;
  datasets: number;
}

export interface PsSearchResult {
  totalDatasets: number;
  datasets: PsHit[];
  unmapped: boolean;
  organismFacet: PsFacetValue[];
}

export interface PsModification {
  name: string;
  residues: string[];
  nDatasets: number;
}

export interface StreamHandle {
  url: string;
  sizeBytes: number;
}

export interface StreamStat {
  sizeBytes: number;
  fetchedBytes: number;
  requests: number;
}

export const api = {
  /** Native file picker for a single QPX file. */
  chooseFile: (): Promise<string | null> =>
    open({
      multiple: false,
      filters: [{ name: "QPX", extensions: ["parquet", "tsv"] }],
    }) as Promise<string | null>,

  /** Native folder picker. */
  chooseFolder: (): Promise<string | null> =>
    open({ directory: true, multiple: false }) as Promise<string | null>,

  openFile: (path: string) => invoke<DatasetInfo>("open_file", { path }),

  scanFolder: (path: string) => invoke<DatasetInfo[]>("scan_folder", { path }),

  scanCollection: (path: string) => invoke<DatasetInfo[]>("scan_collection", { path }),

  listS3: (bucket: string, prefix?: string) =>
    invoke<S3Listing>("list_s3", { bucket, prefix }),

  cacheS3Views: (bucket: string, paths: string[], cancelId?: string) =>
    invoke<ViewInfo[]>("cache_s3_views", { bucket, paths, cancelId }),

  /** Sign S3 paths so DuckDB can read them lazily over HTTP without credentials. */
  presignS3Urls: (bucket: string, paths: string[]) =>
    invoke<Record<string, string>>("presign_s3_urls", { bucket, paths }),
  /** Register S3 views with the backend range proxy (lazy, no download). */
  streamS3Urls: (bucket: string, paths: string[]) =>
    invoke<Record<string, StreamHandle>>("stream_s3_urls", { bucket, paths }),
  /** Register public portal (https) files with the backend range proxy. */
  streamHttpUrls: (urls: string[]) =>
    invoke<Record<string, StreamHandle>>("stream_http_urls", { urls }),
  /** Register local parquet files with the range proxy (lazy reads from disk). */
  streamLocalFiles: (paths: string[]) =>
    invoke<Record<string, StreamHandle>>("stream_local_files", { paths }),
  /** Bytes actually fetched per streamed view (keyed by its s3:// or https path). */
  streamS3Stats: () => invoke<Record<string, StreamStat>>("stream_s3_stats"),

  /** Ask the backend to abort an in-flight download batch. */
  cancelDownload: (id: string) => invoke<void>("cancel_download", { id }),
  /** Free bytes on the download cache disk, and the spare amount always kept free. */
  cacheFreeSpace: (kind: "portal" | "s3") =>
    invoke<{ free: number; reserve: number; dir: string }>("cache_free_space", { kind }),

  readFileBytes: (path: string) => invoke<string>("read_file_bytes", { path }),
  writeFileBytes: (path: string, base64: string) =>
    invoke<void>("write_file_bytes", { path, base64 }),
  /** Native save dialog; resolves to null when cancelled. */
  chooseSavePath: (defaultPath: string, name: string, extensions: string[]) =>
    save({ defaultPath, filters: [{ name, extensions }] }) as Promise<string | null>,

  // ---- quantms portal (public example source over HTTPS) ----
  portalDatasets: (limit?: number) => invoke<string[]>("portal_datasets", { limit }),
  portalRegistry: (pageSize?: number) =>
    invoke<PortalDataset[]>("portal_registry", { pageSize }),
  /** Server-side search over the quantms portal catalogue (q text + facets + sort). */
  portalSearch: (
    query?: string | null,
    opts?: {
      collection?: string | null;
      organism?: string | null;
      instrument?: string | null;
      sort?: string | null;
      page?: number | null;
      pageSize?: number | null;
    }
  ) =>
    invoke<PortalSearchResult>("portal_search", {
      query: query ?? null,
      collection: opts?.collection ?? null,
      organism: opts?.organism ?? null,
      instrument: opts?.instrument ?? null,
      sort: opts?.sort ?? null,
      page: opts?.page ?? null,
      pageSize: opts?.pageSize ?? null,
    }),
  /** Organism / instrument / collection facet options for the discover UI. */
  portalFacets: () => invoke<PortalFacets>("portal_facets"),
  /** Cross-dataset peptide / protein / gene search over the quantms portal. */
  psSearch: (
    query: string,
    opts?: {
      dtype?: string | null;
      matchMode?: string | null;
      organism?: string | null;
      modification?: string | null;
      minLength?: number | null;
      maxLength?: number | null;
      limit?: number | null;
      offset?: number | null;
    }
  ) =>
    invoke<PsSearchResult>("ps_search", {
      query,
      dtype: opts?.dtype ?? null,
      matchMode: opts?.matchMode ?? null,
      organism: opts?.organism ?? null,
      modification: opts?.modification ?? null,
      minLength: opts?.minLength ?? null,
      maxLength: opts?.maxLength ?? null,
      limit: opts?.limit ?? null,
      offset: opts?.offset ?? null,
    }),
  psModifications: () => invoke<PsModification[]>("ps_modifications"),
  portalListViews: (dataset: string, organism?: string | null, psmCount?: number | null) =>
    invoke<ViewInfo[]>("portal_list_views", { dataset, organism: organism ?? null, psmCount: psmCount ?? null }),
  portalCacheViews: (urls: string[], cancelId?: string) =>
    invoke<ViewInfo[]>("portal_cache_views", { urls, cancelId }),
};
