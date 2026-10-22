import { hasExactKeys } from "./guards";
import type { ContainerConstrainedReport, RemotePolicyReport, RunnerCheck } from "./reports";
import { CONTAINER_CONSTRAINED_REPORT_TYPE, REMOTE_POLICY_REPORT_TYPE } from "./scenarios";

const containerReportKeys = new Set([
  "type",
  "source",
  "childScript",
  "epubReadingSystem",
  "parentDomBlocked",
  "frameSizeBlocked",
  "readerDomBlocked",
  "credentialsBlocked",
  "nativeRouteBlocked",
]);
const remotePolicyReportKeys = new Set([
  "type",
  "source",
  "httpsFetch",
  "httpsImage",
  "httpsStyle",
  "httpsScript",
  "httpsAudio",
  "httpsFrame",
  "wssEcho",
  "httpFetch",
  "wsEcho",
]);

const remotePolicyOutcomes = new Set<string>([
  "blocked:connect-src",
  "blocked:img-src",
  "blocked:style-src",
  "blocked:style-src-elem",
  "blocked:script-src",
  "blocked:script-src-elem",
  "blocked:media-src",
  "blocked:frame-src",
  "loaded",
  "failed",
  "indeterminate",
]);

export function parseContainerConstrainedReport(value: unknown): ContainerConstrainedReport | null {
  if (
    !hasExactKeys(value, containerReportKeys) ||
    value.type !== CONTAINER_CONSTRAINED_REPORT_TYPE ||
    value.source !== "EPUB/content.xhtml" ||
    [
      value.childScript,
      value.epubReadingSystem,
      value.parentDomBlocked,
      value.frameSizeBlocked,
      value.readerDomBlocked,
      value.credentialsBlocked,
      value.nativeRouteBlocked,
    ].some((item) => typeof item !== "boolean")
  )
    return null;
  return value as unknown as ContainerConstrainedReport;
}

export function parseRemotePolicyReport(value: unknown): RemotePolicyReport | null {
  if (
    !hasExactKeys(value, remotePolicyReportKeys) ||
    value.type !== REMOTE_POLICY_REPORT_TYPE ||
    value.source !== "EPUB/text/probe.xhtml" ||
    [
      value.httpsFetch,
      value.httpsImage,
      value.httpsStyle,
      value.httpsScript,
      value.httpsAudio,
      value.httpsFrame,
      value.wssEcho,
      value.httpFetch,
      value.wsEcho,
    ].some((item) => typeof item !== "string" || !remotePolicyOutcomes.has(item))
  )
    return null;
  return value as unknown as RemotePolicyReport;
}

export function evaluateContainerConstrained(report: ContainerConstrainedReport): RunnerCheck[] {
  return [
    ["child-script", report.childScript],
    ["epub-reading-system", report.epubReadingSystem],
    ["parent-dom-blocked", report.parentDomBlocked],
    ["frame-size-blocked", report.frameSizeBlocked],
    ["reader-dom-blocked", report.readerDomBlocked],
    ["credentials-blocked", report.credentialsBlocked],
    ["native-route-blocked", report.nativeRouteBlocked],
  ].map(([label, passed]) => ({
    label: String(label),
    passed: Boolean(passed),
    detail: String(passed),
  }));
}

export function evaluateRemotePolicyDenied(report: RemotePolicyReport): RunnerCheck[] {
  const expected: readonly [
    keyof Omit<RemotePolicyReport, "type" | "source">,
    readonly string[],
  ][] = [
    ["httpsFetch", ["blocked:connect-src"]],
    ["httpsImage", ["blocked:img-src"]],
    ["httpsStyle", ["blocked:style-src", "blocked:style-src-elem"]],
    ["httpsScript", ["blocked:script-src", "blocked:script-src-elem"]],
    ["httpsAudio", ["blocked:media-src"]],
    ["httpsFrame", ["blocked:frame-src"]],
    ["wssEcho", ["blocked:connect-src"]],
    ["httpFetch", ["blocked:connect-src"]],
    ["wsEcho", ["blocked:connect-src"]],
  ];
  return expected.map(([field, outcomes]) => ({
    label: `remote-policy-${field}`,
    passed: outcomes.includes(report[field]),
    detail: report[field],
  }));
}
