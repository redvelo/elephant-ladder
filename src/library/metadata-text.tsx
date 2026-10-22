import { type MetadataDirection } from "../host/bootstrap/types";

/**
 * Publication metadata rendered the way its publication says it reads.
 *
 * A declared direction has to be honoured rather than inferred: a right-to-left name
 * written in Latin letters infers left-to-right, and bidirectional text around a Latin
 * prefix reorders differently depending on the base direction. Where nothing is declared
 * the text is inferred from rather than inheriting the application's direction, which is
 * about the interface and says nothing about the publication.
 */
export function MetadataText({
  text,
  direction,
}: {
  text: string;
  direction: MetadataDirection | null;
}) {
  return <span dir={direction ?? "auto"}>{text}</span>;
}

/** Each creator carries its own direction, so they are separate runs rather than one
 * joined string. */
export function CreatorList({
  creators,
  directions,
}: {
  creators: string[];
  directions: (MetadataDirection | null)[];
}) {
  return (
    <>
      {creators.map((creator, index) => (
        <span key={`${creator}-${index}`}>
          {index > 0 ? ", " : null}
          <MetadataText text={creator} direction={directions[index] ?? null} />
        </span>
      ))}
    </>
  );
}
