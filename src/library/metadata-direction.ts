import { type MetadataDirection } from "../host/bootstrap/types";

/**
 * The direction a block of creators reads as a whole.
 *
 * Direction does two things, and only one of them is inline. Reordering runs happens
 * inside the element that carries the attribute, but the base alignment of the line comes
 * from the block, so a name with no right-to-left characters shows its direction only by
 * sitting at the right. A block has to pick one direction, so it takes the one they all
 * share and otherwise leaves it to be inferred.
 */
export function creatorsDirection(
  directions: (MetadataDirection | null)[],
): MetadataDirection | null {
  const [first, ...rest] = directions;
  if (first === null || first === undefined) return null;
  return rest.every((direction) => direction === first) ? first : null;
}
