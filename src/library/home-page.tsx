import { Button as BaseButton } from "@base-ui/react/button";
import type { CSSProperties } from "react";
import { type PublicationSummary } from "../host/bootstrap/types";
import { dateTimestamp } from "../lib/format";
import { CreatorList, MetadataText } from "./metadata-text";
import { creatorsDirection } from "./metadata-direction";
import { reflectionPrompts, summaryPages } from "../journal/model";
import type { JournalState } from "../journal/state";
import { ReadingActivityRow } from "../reading-activity/row";
import type { BookInteractions } from "./actions";
import { BookCover } from "./book-cover";
import { BookMenu, type BookAction } from "./book-menu";
import { usePublicationDescription } from "./publication-description";
import { useFittedLines } from "./use-fitted-lines";
import "./home.css";

export function HomePage({
  featured,
  publications,
  preparing,
  journalState,
  portalContainer,
  onBookAction,
  onReflection,
  ...actions
}: BookInteractions & {
  featured: PublicationSummary;
  publications: readonly PublicationSummary[];
  /** Opening ends in a navigation, so only the slow preparation before it is reported. */
  preparing: boolean;
  journalState: JournalState | undefined;
  portalContainer: HTMLElement | null;
  onBookAction: (action: BookAction, publication: PublicationSummary, trigger: HTMLElement) => void;
  onReflection: (trigger: HTMLElement) => void;
}) {
  const journal = journalState?.status === "ready" ? journalState.journal : null;
  const latest = journal?.draft ?? journal?.entries[0];
  const pages = summaryPages(journal?.summary ?? null);
  const description = usePublicationDescription(featured.publicationId, actions.sessionToken);
  const [measureDescription, descriptionLines] = useFittedLines();
  const recent = publications
    .filter((book) => book.publicationId !== featured.publicationId && book.lastReadAt !== null)
    .sort((a, b) => dateTimestamp(b.lastReadAt) - dateTimestamp(a.lastReadAt))
    .slice(0, 6);
  return (
    <div className="home-page" data-has-recents={recent.length > 0}>
      <div className="home-current-book">
        <section className="home-hero" aria-labelledby="current-book-title">
          <BookMenu
            publication={featured}
            sessionToken={actions.sessionToken}
            disabled={actions.disabled}
            container={portalContainer}
            className="hero-cover-button"
            onAction={onBookAction}
            onOpen={() => actions.onOpen(featured)}
          >
            <BookCover publication={featured} sessionToken={actions.sessionToken} />
          </BookMenu>
          <div className="hero-copy">
            <h1 id="current-book-title" dir={featured.titleDirection ?? "auto"}>
              <BaseButton
                className="hero-title-button"
                disabled={actions.disabled}
                onClick={() => actions.onOpen(featured)}
              >
                <MetadataText text={featured.title} direction={featured.titleDirection} />
              </BaseButton>
            </h1>
            <p
              className="hero-author"
              dir={creatorsDirection(featured.creatorDirections) ?? "auto"}
            >
              <CreatorList creators={featured.creators} directions={featured.creatorDirections} />
            </p>
            <div className="hero-reading-context">
              <span>
                {journal?.summary?.chapter ??
                  (featured.hasReadingPosition
                    ? "Pick up where you left off"
                    : "Ready when you are")}
              </span>
              {pages !== null ? <span>{pages}</span> : null}
            </div>
            <div className="hero-actions">
              {description !== null ? (
                <p
                  ref={measureDescription}
                  className="hero-description"
                  style={
                    descriptionLines === null
                      ? undefined
                      : ({ "--hero-description-lines": descriptionLines } as CSSProperties)
                  }
                >
                  {description}
                </p>
              ) : null}
              {preparing ? <span role="status">Preparing…</span> : null}
            </div>
          </div>
        </section>
        <div className="home-reflection-row">
          {journalState?.status === "failed" ? <p role="status">{journalState.message}</p> : null}
          <BaseButton
            className="home-reflection"
            disabled={actions.disabled || journalState?.status === "loading"}
            onClick={(event) => onReflection(event.currentTarget)}
          >
            <span className="home-reflection-excerpt">
              {latest?.text || reflectionPrompts[latest?.prompt ?? 0]}
            </span>
            <span className="home-reflection-caption">
              <span className="home-reflection-caption-label">
                {latest?.text
                  ? journal?.draft
                    ? "Continue draft"
                    : "View reflections"
                  : "Add a reflection"}
              </span>
              <span aria-hidden="true">→</span>
            </span>
          </BaseButton>
        </div>
      </div>
      <ReadingActivityRow sessionToken={actions.sessionToken} />
      {recent.length > 0 ? (
        <section className="home-recents" aria-label="Recently opened">
          <ol className="recent-books">
            {recent.map((book) => (
              <li key={book.publicationId}>
                <BookMenu
                  publication={book}
                  sessionToken={actions.sessionToken}
                  disabled={actions.disabled}
                  container={portalContainer}
                  onAction={onBookAction}
                  onOpen={() => actions.onOpen(book)}
                >
                  <BookCover publication={book} sessionToken={actions.sessionToken} />
                </BookMenu>
              </li>
            ))}
          </ol>
        </section>
      ) : null}
    </div>
  );
}
