import type { NavigatorLocationSnapshot } from "epub-navigator";
import type { NavigatorReadingSummary } from "epub-navigator/react";
import { atom } from "jotai";
import { checkpointReadingPosition, requestReadingPositionCapture } from "./persistence/position";
import { replaceReadingPositionTimer } from "./reading-position";
import { positionIssues, positionOwnerAtom, positionOwnerRef, readerRef } from "./state";

export const recordReaderLocationAtom = atom(
  null,
  (get, _set, snapshot: NavigatorLocationSnapshot): void => {
    const owner = get(positionOwnerAtom);
    if (owner === null || owner.stopping) return;
    owner.debounceTimer = replaceReadingPositionTimer(window, owner.debounceTimer, snapshot, () => {
      owner.debounceTimer = null;
      requestReadingPositionCapture(
        owner,
        readerRef(get),
        positionOwnerRef(get),
        positionIssues(get),
      );
    });
  },
);

export const recordReaderSummaryAtom = atom(
  null,
  (get, _set, summary: NavigatorReadingSummary): void => {
    const owner = get(positionOwnerAtom);
    if (
      owner === null ||
      owner.stopping ||
      JSON.stringify(owner.summary) === JSON.stringify(summary)
    )
      return;
    owner.summary = summary;
    requestReadingPositionCapture(
      owner,
      readerRef(get),
      positionOwnerRef(get),
      positionIssues(get),
    );
  },
);

export const checkpointReadingPositionAtom = atom(null, (get): void => {
  const owner = get(positionOwnerAtom);
  if (owner === null || owner.stopping) return;
  void checkpointReadingPosition(owner, readerRef(get), positionOwnerRef(get), positionIssues(get));
});
