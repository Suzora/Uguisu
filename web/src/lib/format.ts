// Formatting the UI does the same way everywhere. Nothing here decides
// anything: the backend owns every state name and number these render.
//
// Dates and numbers follow the catalogue's locale, never the system's: the
// system locale would put German dates and "vor 5 Minuten" into an English
// interface (ADR 0053).
import { locale, m } from "./i18n";

const UNITS = ["B", "KiB", "MiB", "GiB", "TiB"];

/** A byte count a person can read, e.g. `48.2 MiB`. */
export function bytes(value: number | null | undefined): string {
  if (value === null || value === undefined) {
    return "—";
  }
  let size = value;
  let unit = 0;
  while (size >= 1024 && unit < UNITS.length - 1) {
    size /= 1024;
    unit += 1;
  }
  const digits = unit === 0 || size >= 100 ? 0 : 1;
  const number = new Intl.NumberFormat(locale, {
    minimumFractionDigits: digits,
    maximumFractionDigits: digits,
    useGrouping: false,
  });
  return `${number.format(size)} ${UNITS[unit]}`;
}

/** A transfer rate, e.g. `1.4 MiB/s`. */
export function rate(bytesPerSecond: number | null | undefined): string {
  return bytesPerSecond ? `${bytes(bytesPerSecond)}/s` : "—";
}

/** A duration in seconds as `h:mm:ss` or `m:ss`. */
export function duration(seconds: number | null | undefined): string {
  if (seconds === null || seconds === undefined || seconds < 0) {
    return "—";
  }
  const whole = Math.floor(seconds);
  const h = Math.floor(whole / 3600);
  const m = Math.floor((whole % 3600) / 60);
  const s = whole % 60;
  const pad = (n: number) => String(n).padStart(2, "0");
  return h > 0 ? `${h}:${pad(m)}:${pad(s)}` : `${m}:${pad(s)}`;
}

/** A countdown, e.g. `about 3 min left`; `—` when there is no estimate. */
export function eta(seconds: number | null | undefined): string {
  if (seconds === null || seconds === undefined) {
    return "—";
  }
  if (seconds < 60) {
    return m.common.format.secondsLeft(Math.max(1, Math.round(seconds)));
  }
  return m.common.format.minutesLeft(Math.round(seconds / 60));
}

/** A timestamp in the viewer's time zone, e.g. `Jan 15, 2026, 1:05:00 PM`, or `—`. */
export function dateTime(iso: string | null | undefined): string {
  if (!iso) {
    return "—";
  }
  const at = new Date(iso);
  return Number.isNaN(at.getTime())
    ? iso
    : at.toLocaleString(locale, { dateStyle: "medium", timeStyle: "medium" });
}

/** Just the day, e.g. `Jan 15, 2026`, for episode lists. */
export function date(iso: string | null | undefined): string {
  if (!iso) {
    return "—";
  }
  const at = new Date(iso);
  return Number.isNaN(at.getTime())
    ? iso
    : at.toLocaleDateString(locale, { dateStyle: "medium" });
}

const STEPS: [Intl.RelativeTimeFormatUnit, number][] = [
  ["second", 60],
  ["minute", 60],
  ["hour", 24],
  ["day", 7],
  ["week", 4.35],
  ["month", 12],
];

/** `in 4 minutes` / `2 days ago`, relative to `now`. */
export function relative(
  iso: string | null | undefined,
  now = Date.now(),
): string {
  if (!iso) {
    return "—";
  }
  const at = new Date(iso).getTime();
  if (Number.isNaN(at)) {
    return iso;
  }
  const format = new Intl.RelativeTimeFormat(locale, { numeric: "auto" });
  let value = (at - now) / 1000;
  for (const [unit, step] of STEPS) {
    if (Math.abs(value) < step) {
      return format.format(Math.round(value), unit);
    }
    value /= step;
  }
  return format.format(Math.round(value), "year");
}

/** A share of a whole as a percentage, or `null` when the whole is unknown. */
export function percentage(
  done: number,
  total: number | null | undefined,
): number | null {
  if (!total || total <= 0) {
    return null;
  }
  return Math.min(100, Math.round((done / total) * 1000) / 10);
}

/** `11.4 MiB of 45.8 MiB (25%)`, or just the amount when the total is unknown. */
export function transferred(
  done: number,
  total: number | null | undefined,
): string {
  const share = percentage(done, total);
  return share === null
    ? bytes(done)
    : m.common.format.transferred(bytes(done), bytes(total), share);
}

/**
 * `download.progress` → `Download progress`, for the activity feed: the
 * catalogue's word for the kind, or the kind with spaces.
 */
export function eventLabel(kind: string): string {
  const words = m.values[kind] ?? kind.replace(/[._]/g, " ");
  return words.charAt(0).toUpperCase() + words.slice(1);
}
