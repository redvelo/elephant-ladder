import type { ReactNode } from "react";
import type { Destination } from "../app/destination";

export const destinations: { id: Destination; label: string; path: ReactNode }[] = [
  {
    id: "home",
    label: "Home",
    path: (
      <>
        <path d="m3 10 9-7 9 7v11H3Z" />
        <path d="M9 21v-8h6v8" />
      </>
    ),
  },
  {
    id: "library",
    label: "Library",
    path: (
      <>
        <path d="M3 4h5v17H3zM10 4h5v17h-5zM17 5l4-1 3 16-4 1z" />
      </>
    ),
  },
  {
    id: "settings",
    label: "Settings",
    path: (
      <>
        <path d="M4 6h16M4 12h16M4 18h16" />
        <path d="M8 3v6M16 9v6M10 15v6" />
      </>
    ),
  },
];
