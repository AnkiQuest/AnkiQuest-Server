/* Small helpers every bundle needs, imported rather than shared through `window`
   so they also work in scripts that run before site.js. */

const entities: Record<string, string> = {"&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;"};

/** Text safe to place in HTML content or a quoted attribute; `null` and `undefined` become empty. */
export function escape(value: unknown): string {
  return String(value ?? "").replace(/[&<>"']/g, char => entities[char]!);
}

/**
 * The account AnkiDroid handed this page, or `null` when it has none or is signed out. Current apps
 * hand only the member and have already installed its HttpOnly session cookie, so requests for it
 * carry no token; older apps also hand the token itself.
 */
export function nativeAccount(): OwnerSession | null {
  const value = window.ankiquestSession;
  if (typeof value?.user !== "string" || !value.user) return null;
  const token = typeof value.token === "string" ? value.token.trim() : "";
  return token ? {user: value.user, token} : {user: value.user};
}

/**
 * Up to two initials for an avatar, from the display name (or the user when it has none). They keep
 * the name's own case: hill writes his name in lowercase, so his avatar shows "h".
 */
export function initials(user: string, display = user): string {
  return String(display || user).trim().split(/\s+/).map(part => Array.from(part)[0] || "").slice(0, 2).join("");
}

/** A count in the page's language; anything that is not a number shows as 0. */
export function number(value: unknown): string {
  return Number(value || 0).toLocaleString(AnkiQuestI18n.language);
}

/** Orders strings by code point, the same in every browser and language. */
export function lexical(a: string, b: string): number {
  return a < b ? -1 : a > b ? 1 : 0;
}

/**
 * The heatmap's colour level 0-4 of a day with `count` reviews, measured against the
 * average per study day: none, up to half the average, the average, twice it, more.
 * `review_heatmap.json`'s `intensity_cases` pin the levels; both clients colour their
 * heatmaps with the same rule.
 */
export function heatmapIntensity(count: number, dailyAverage: number): number {
  const average = Math.max(dailyAverage, 1);
  if (count <= 0) return 0;
  if (count <= average / 2) return 1;
  if (count <= average) return 2;
  if (count <= average * 2) return 3;
  return 4;
}

/** The 24 `<option>`s of an hour picker, 00:00 to 23:00, with `selected` chosen. */
export function hourOptions(selected: unknown): string {
  return Array.from({length: 24}, (_, hour) => `<option value="${hour}" ${hour === Number(selected) ? "selected" : ""}>${String(hour).padStart(2, "0")}:00</option>`).join("");
}

/**
 * A calendar day such as "2026-09-25" in the page's language, read at noon UTC so no time zone
 * moves it to another day: "—" for none, and the value itself when it is not a date.
 */
export function day(value: unknown, options: Intl.DateTimeFormatOptions = {month: "short", day: "numeric", year: "numeric"}): string {
  if (!value) return "—";
  const parsed = new Date(String(value).slice(0, 10) + "T12:00:00Z");
  return Number.isNaN(parsed.getTime()) ? String(value) : new Intl.DateTimeFormat(AnkiQuestI18n.language, {...options, timeZone: "UTC"}).format(parsed);
}

/** The closing week, still counting the player's current day (`ClosingWeek` in src/game.rs). */
export interface ClosingWeek { xp: number; ends_at: number; }

/** The week the board switches to next, already counting the player's current day (`NextWeek` in src/game.rs). */
export interface NextWeek { xp: number; starts_at: number; }

/**
 * The notice for the other week the player's current day counts toward: the week the board has
 * just left, until that day ends, or the week it has yet to reach, until the board switches.
 * Empty when neither applies. `classes` adds the page's layout classes.
 */
export function weekNotice(closing: ClosingWeek | null | undefined, next: NextWeek | null | undefined, classes = ""): string {
  const clock = (at: number) => new Date(at).toLocaleTimeString(AnkiQuestI18n.language, {hour: "2-digit", minute: "2-digit"});
  const notice = (id: string, text: string) => `<p class="${["notice", id, classes].filter(Boolean).join(" ")}" id="${id}" role="status">${escape(text)}</p>`;
  const {t} = AnkiQuestI18n;

  // The board has either reset or not, so at most one of the two applies.
  if (closing && Number(closing.ends_at) > Date.now()) {
    return notice("closing-week", t`Last week still counting: ${number(closing.xp)} XP · closes at ${clock(Number(closing.ends_at))}`);
  }
  if (next && Number(next.starts_at) > Date.now()) {
    return notice("next-week", t`Next week already counting: ${number(next.xp)} XP · the board switches at ${clock(Number(next.starts_at))}`);
  }
  return "";
}
