export type WebsiteDataPurgeCategoryStatus = "attempted" | "unsupported" | "failed";

export interface WebsiteDataPurgeReport {
  readonly androidWebStorage: WebsiteDataPurgeCategoryStatus;
  readonly cookies: WebsiteDataPurgeCategoryStatus;
  readonly cacheStorage: WebsiteDataPurgeCategoryStatus;
  readonly workerStorage: WebsiteDataPurgeCategoryStatus;
  readonly httpAuthentication: WebsiteDataPurgeCategoryStatus;
  readonly formData: WebsiteDataPurgeCategoryStatus;
  readonly opfs: WebsiteDataPurgeCategoryStatus;
}

export interface WebsiteDataResetResponse {
  readonly status: "reset";
  readonly physicalPurge: "partial" | "unsupported";
  readonly purgeReport: WebsiteDataPurgeReport;
}

const purgeReportKeys = [
  "androidWebStorage",
  "cookies",
  "cacheStorage",
  "workerStorage",
  "httpAuthentication",
  "formData",
  "opfs",
] as const;

function isPurgeCategoryStatus(value: unknown): value is WebsiteDataPurgeCategoryStatus {
  return value === "attempted" || value === "unsupported" || value === "failed";
}

function isPurgeReport(value: unknown): value is WebsiteDataPurgeReport {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
  const report = value as Readonly<Record<string, unknown>>;
  return (
    Object.keys(report).length === purgeReportKeys.length &&
    purgeReportKeys.every((key) => isPurgeCategoryStatus(report[key]))
  );
}

export function isWebsiteDataResetResponse(value: unknown): value is WebsiteDataResetResponse {
  return (
    typeof value === "object" &&
    value !== null &&
    !Array.isArray(value) &&
    Object.keys(value).length === 3 &&
    "status" in value &&
    value.status === "reset" &&
    "physicalPurge" in value &&
    (value.physicalPurge === "partial" || value.physicalPurge === "unsupported") &&
    "purgeReport" in value &&
    isPurgeReport(value.purgeReport)
  );
}
