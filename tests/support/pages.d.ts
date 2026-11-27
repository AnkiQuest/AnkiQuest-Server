// What tests reach inside browser pages: the globals the web scripts share, and the
// top-level renderers of page scripts loaded with `prefix` from ./web.ts.
/// <reference path="../../web/globals.d.ts" />

export {};

declare global {
  // web/pages/index.ts
  var app: HTMLElement;
  function board(rows: object[], me: string): string;
  var weekEnds: number;
  function heat(cells: object[], lifetime: {reviews: number; days_active: number}): string;
  function view(profile: object, rows: object[], canEditAvatar?: boolean): string;
  // web/common.ts, bundled into the index page
  function weekNotice(closing: {xp: number; ends_at: number} | undefined, next: {xp: number; starts_at: number} | undefined, classes?: string): string;
  // web/pages/community.ts
  var state: {data: unknown};
  function avatar(user: string, display?: string): string;
  function renderOverview(): void;
  function renderYear(): void;
}
