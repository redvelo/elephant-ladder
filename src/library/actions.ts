import { useSetAtom } from "jotai";
import type { RefObject } from "react";
import {
  annotationImportRequestAtom,
  copyAnnotationsAtom,
  exportAnnotationsAtom,
} from "../annotations/dispatch";
import { type PublicationSummary } from "../host/bootstrap/types";
import type { BookAction } from "./book-menu";
import { confirmDeletionAtom } from "./deletion";
import { showBookDetailsAtom, showReflectionsAtom } from "./dispatch";
import { resetPublicationLayoutAtom } from "./layout-reset";
import { returnFocusAtom } from "./shell-state";

export type BookActionHandler = (
  action: BookAction,
  publication: PublicationSummary,
  trigger: HTMLElement,
) => void;

/** What a book's menu can ask for, bound to the stores that carry it out. */
export function useBookAction(
  annotationInput: RefObject<HTMLInputElement | null>,
): BookActionHandler {
  const setReturnFocus = useSetAtom(returnFocusAtom);
  const showReflections = useSetAtom(showReflectionsAtom);
  const setAnnotationImportRequest = useSetAtom(annotationImportRequestAtom);
  const resetPublicationLayout = useSetAtom(resetPublicationLayoutAtom);
  const showBookDetails = useSetAtom(showBookDetailsAtom);
  const copyAnnotations = useSetAtom(copyAnnotationsAtom);
  const exportAnnotations = useSetAtom(exportAnnotationsAtom);
  const confirmDeletion = useSetAtom(confirmDeletionAtom);
  return (action, publication, trigger) => {
    setReturnFocus(trigger);
    if (action === "reflections") showReflections(publication, trigger);
    else if (action === "default-layout") void resetPublicationLayout(publication);
    else if (action === "site-data") showBookDetails(publication, trigger);
    else if (action === "copy-annotations") void copyAnnotations(publication);
    else if (action === "export-annotations") void exportAnnotations(publication);
    else if (action === "import-annotations") {
      setAnnotationImportRequest(publication);
      annotationInput.current?.click();
    } else confirmDeletion(publication);
  };
}

/** What every book surface needs to open a book, and whether it may. */
export interface BookInteractions {
  sessionToken: string;
  disabled: boolean;
  onOpen: (publication: PublicationSummary) => void;
}
